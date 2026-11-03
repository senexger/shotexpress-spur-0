//! A trip started from the admin panel: drive to a station, and if the
//! station says so (`then` / `wait_s` in track.toml) wait there and drive
//! on. Raucherecke → 2 min → Tankstation, which waits until the next click.
//! A new trip or any manual control cancels the running one at once.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tokio::task::JoinHandle;

use crate::orders::drive;
use crate::service::TrainService;
use crate::track::TrackConfig;
use crate::types::now_ms;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TripStatus {
    Idle,
    Driving { target: String },
    /// Waiting at `at`, leaves for `next` at `until_ms` (`left_s` from now).
    Waiting { at: String, next: String, until_ms: u64, left_s: u64 },
    Arrived { at: String },
    Failed { target: String, reason: String },
    Cancelled { reason: String },
}

pub struct Trip {
    status: Mutex<TripStatus>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl Trip {
    pub fn new() -> Arc<Self> {
        Arc::new(Trip {
            status: Mutex::new(TripStatus::Idle),
            task: Mutex::new(None),
        })
    }

    pub fn status(&self) -> TripStatus {
        let mut status = self.status.lock().expect("trip status poisoned").clone();
        if let TripStatus::Waiting { until_ms, left_s, .. } = &mut status {
            *left_s = until_ms.saturating_sub(now_ms()).div_ceil(1000);
        }
        status
    }

    fn set(&self, status: TripStatus) {
        *self.status.lock().expect("trip status poisoned") = status;
    }

    fn abort(&self) -> bool {
        match self.task.lock().expect("trip task poisoned").take() {
            Some(task) if !task.is_finished() => {
                task.abort();
                true
            }
            _ => false,
        }
    }

    /// Start a trip to `target`, replacing any running one.
    pub fn start(
        self: &Arc<Self>,
        service: Arc<TrainService>,
        track: Arc<TrackConfig>,
        target: String,
    ) {
        self.abort();
        // the old ride is replaced: the train cancels it on the new drive
        service.release_current();
        self.set(TripStatus::Driving {
            target: target.clone(),
        });
        let trip = self.clone();
        let task = tokio::spawn(async move { trip.run(&service, &track, target).await });
        *self.task.lock().expect("trip task poisoned") = Some(task);
    }

    /// Manual control took over: stop the trip where it is.
    pub fn cancel(&self, reason: &str) {
        if self.abort() {
            tracing::info!(reason, "trip cancelled");
            self.set(TripStatus::Cancelled {
                reason: reason.to_string(),
            });
        }
    }

    async fn run(&self, service: &TrainService, track: &TrackConfig, mut target: String) {
        loop {
            self.set(TripStatus::Driving {
                target: target.clone(),
            });
            tracing::info!(%target, "trip: driving");
            if let Err(reason) = drive(service, track, &target).await {
                tracing::warn!(%target, %reason, "trip failed");
                self.set(TripStatus::Failed { target, reason });
                return;
            }
            let next = track.station(&target).and_then(|s| {
                s.then.clone().map(|next| (next, Duration::from_secs(s.wait_s)))
            });
            let Some((next, wait)) = next else {
                tracing::info!(%target, "trip: arrived, waiting for the next click");
                self.set(TripStatus::Arrived { at: target });
                return;
            };
            tracing::info!(%target, %next, wait_s = wait.as_secs(), "trip: arrived, waiting");
            self.set(TripStatus::Waiting {
                at: target.clone(),
                next: next.clone(),
                until_ms: now_ms() + wait.as_millis() as u64,
                left_s: wait.as_secs(),
            });
            tokio::time::sleep(wait).await;
            target = next;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::tests::{complete_current, service};

    fn track() -> Arc<TrackConfig> {
        let track: TrackConfig = toml::from_str(
            r#"
[train]
max_speed = 1.0
start_speed = 0.3
accel = 0.5
max_run_ms_without_tag = 30000
default_ttl_ms = 60000

[refill]
station = "Tankstation"
steps = 1000
speed = 0.2

[[station]]
name = "Tankstation"
tag = "04402852803C80"
role = "refill"

[[station]]
name = "Raucherecke"
tag = "041A45D2462490"
role = "unload"
wait_s = 120
then = "Tankstation"
"#,
        )
        .unwrap();
        Arc::new(track)
    }

    /// Let the trip task run until it has sent its next ride, then finish it.
    async fn arrive(service: &TrainService) {
        for _ in 0..100 {
            if complete_current(service) {
                // let the trip see the event
                for _ in 0..10 {
                    tokio::task::yield_now().await;
                }
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("the trip never sent a ride");
    }

    #[tokio::test(start_paused = true)]
    async fn raucherecke_waits_two_minutes_then_goes_to_the_tankstation() {
        let (service, _eventloop) = service();
        let track = track();
        service.set_track_table(track.table());
        let trip = Trip::new();

        trip.start(service.clone(), track, "Raucherecke".into());
        assert!(matches!(trip.status(), TripStatus::Driving { ref target } if target == "Raucherecke"));

        arrive(&service).await;
        match trip.status() {
            TripStatus::Waiting { at, next, left_s, .. } => {
                assert_eq!((at.as_str(), next.as_str(), left_s), ("Raucherecke", "Tankstation", 120));
            }
            other => panic!("expected waiting, got {other:?}"),
        }

        tokio::time::sleep(Duration::from_secs(119)).await;
        assert!(matches!(trip.status(), TripStatus::Waiting { .. }), "still waiting");
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(matches!(trip.status(), TripStatus::Driving { ref target } if target == "Tankstation"));

        arrive(&service).await;
        assert_eq!(trip.status(), TripStatus::Arrived { at: "Tankstation".into() });

        // the Tankstation waits for good: no further ride
        tokio::time::sleep(Duration::from_secs(3600)).await;
        assert_eq!(trip.status(), TripStatus::Arrived { at: "Tankstation".into() });
    }

    #[tokio::test(start_paused = true)]
    async fn manual_control_cancels_the_wait() {
        let (service, _eventloop) = service();
        let track = track();
        service.set_track_table(track.table());
        let trip = Trip::new();

        trip.start(service.clone(), track, "Raucherecke".into());
        arrive(&service).await;
        trip.cancel("manual control");
        assert!(matches!(trip.status(), TripStatus::Cancelled { .. }));

        tokio::time::sleep(Duration::from_secs(300)).await;
        assert!(matches!(trip.status(), TripStatus::Cancelled { .. }), "never drives on");
    }

    #[tokio::test(start_paused = true)]
    async fn a_new_click_replaces_the_running_trip() {
        let (service, _eventloop) = service();
        let track = track();
        service.set_track_table(track.table());
        let trip = Trip::new();

        trip.start(service.clone(), track.clone(), "Raucherecke".into());
        arrive(&service).await;
        // clicked again while waiting: drives to Raucherecke anew, not on
        trip.start(service.clone(), track, "Raucherecke".into());
        arrive(&service).await;
        assert!(matches!(trip.status(), TripStatus::Waiting { left_s: 120, .. }));
    }
}
