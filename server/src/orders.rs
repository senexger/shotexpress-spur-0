//! Shot orders from the public order page: a FIFO queue worked off by one
//! task, one order at a time. Each order is a server-side sequence:
//! drive to the refill station (unless the train already stands there),
//! pour one shot per glass with a `steps` nudge between glasses, then drive
//! to the ordered station.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use serde::Serialize;
use tokio::sync::{Notify, broadcast};
use uuid::Uuid;

use crate::service::{FUEL_TTL_MS, TrainService};
use crate::track::TrackConfig;
use crate::types::{
    CmdId, CommandKind, Direction, ExecKind, FuelCommandKind, FuelEventKind, TrainServiceEvent,
    now_ms,
};

/// Finished orders kept for the page to show.
const KEEP_FINISHED: usize = 30;
/// One pour, from command to terminal event.
const POUR_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Queued,
    ToRefill,
    Fueling,
    Delivering,
    Arrived,
    Failed,
}

impl Phase {
    fn is_done(self) -> bool {
        matches!(self, Phase::Arrived | Phase::Failed)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Order {
    /// Short train number shown on the page ("SE 7").
    pub id: u32,
    pub from: String,
    pub target: String,
    pub shots: u32,
    pub poured: u32,
    pub phase: Phase,
    pub error: Option<String>,
    /// The phase an order was in when it failed.
    pub failed_in: Option<Phase>,
    pub created_ms: u64,
    pub updated_ms: u64,
    /// Orders ahead of this one (0 = this one is next or running).
    pub ahead: usize,
}

#[derive(Default)]
struct Book {
    next_id: u32,
    orders: VecDeque<Order>,
}

pub struct OrderQueue {
    book: Mutex<Book>,
    wake: Notify,
    /// Fired by `cancel_all`: the running order gives up at once.
    cancel: Notify,
}

impl OrderQueue {
    pub fn new() -> Arc<Self> {
        Arc::new(OrderQueue {
            book: Mutex::new(Book {
                next_id: 1,
                orders: VecDeque::new(),
            }),
            wake: Notify::new(),
            cancel: Notify::new(),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Book> {
        self.book.lock().expect("order book poisoned")
    }

    pub fn place(&self, from: &str, target: &str, shots: u32) -> Order {
        let mut book = self.lock();
        let id = book.next_id;
        book.next_id += 1;
        let now = now_ms();
        book.orders.push_back(Order {
            id,
            from: from.to_string(),
            target: target.to_string(),
            shots,
            poured: 0,
            phase: Phase::Queued,
            error: None,
            failed_in: None,
            created_ms: now,
            updated_ms: now,
            ahead: 0,
        });
        // drop the oldest finished orders beyond the history limit
        while book.orders.iter().filter(|o| o.phase.is_done()).count() > KEEP_FINISHED {
            let Some(pos) = book.orders.iter().position(|o| o.phase.is_done()) else {
                break;
            };
            book.orders.remove(pos);
        }
        drop(book);
        self.wake.notify_one();
        self.get(id).expect("order just placed")
    }

    pub fn get(&self, id: u32) -> Option<Order> {
        self.list().into_iter().find(|o| o.id == id)
    }

    /// All known orders, newest first, with queue positions filled in.
    pub fn list(&self) -> Vec<Order> {
        let book = self.lock();
        let mut ahead = 0;
        let mut out: Vec<Order> = book
            .orders
            .iter()
            .map(|o| {
                let mut o = o.clone();
                if !o.phase.is_done() {
                    o.ahead = ahead;
                    ahead += 1;
                }
                o
            })
            .collect();
        out.reverse();
        out
    }

    fn next_queued(&self) -> Option<Order> {
        self.lock()
            .orders
            .iter()
            .find(|o| o.phase == Phase::Queued)
            .cloned()
    }

    fn update(&self, id: u32, f: impl FnOnce(&mut Order)) {
        let mut book = self.lock();
        if let Some(order) = book.orders.iter_mut().find(|o| o.id == id) {
            f(order);
            order.updated_ms = now_ms();
        }
    }

    fn set_phase(&self, id: u32, phase: Phase) {
        self.update(id, |o| o.phase = phase);
    }

    /// Manual stop: fail the running order and everything still queued.
    /// Returns how many orders were cancelled.
    pub fn cancel_all(&self, reason: &str) -> usize {
        let mut book = self.lock();
        let now = now_ms();
        let mut count = 0;
        for o in book.orders.iter_mut().filter(|o| !o.phase.is_done()) {
            o.failed_in = Some(o.phase);
            o.phase = Phase::Failed;
            o.error = Some(reason.to_string());
            o.updated_ms = now;
            count += 1;
        }
        drop(book);
        self.cancel.notify_waiters();
        count
    }
}

/// Works off the queue forever, one order at a time.
pub async fn run_worker(
    queue: Arc<OrderQueue>,
    service: Arc<TrainService>,
    track: Arc<RwLock<Arc<TrackConfig>>>,
) {
    loop {
        // listen for a cancel before picking the order: a cancel that comes
        // earlier has already marked it failed, so it is never picked
        let cancelled = queue.cancel.notified();
        tokio::pin!(cancelled);
        cancelled.as_mut().enable();
        let Some(order) = queue.next_queued() else {
            queue.wake.notified().await;
            continue;
        };
        let track = track.read().expect("track lock poisoned").clone();
        tracing::info!(id = order.id, target = %order.target, shots = order.shots, "order started");
        let result = tokio::select! {
            result = run_order(&queue, &service, &track, &order) => result,
            _ = &mut cancelled => {
                tracing::warn!(id = order.id, "order cancelled by manual stop");
                continue;
            }
        };
        match result {
            Ok(()) => {
                queue.set_phase(order.id, Phase::Arrived);
                tracing::info!(id = order.id, "order delivered");
            }
            Err(reason) => {
                tracing::warn!(id = order.id, %reason, "order failed");
                queue.update(order.id, |o| {
                    o.failed_in = Some(o.phase);
                    o.phase = Phase::Failed;
                    o.error = Some(reason);
                });
            }
        }
    }
}

async fn run_order(
    queue: &OrderQueue,
    service: &TrainService,
    track: &TrackConfig,
    order: &Order,
) -> Result<(), String> {
    let refill = &track.refill;
    let snapshot = service.snapshot();
    if !snapshot.train_online {
        return Err("Der Zug ist nicht erreichbar.".into());
    }

    if !refill.auto_fill {
        // filled by hand: straight to the guest
        queue.set_phase(order.id, Phase::Delivering);
        return drive(service, track, &order.target).await;
    }

    // 1. to the bar, unless the last tag the train read is the bar's
    let bar_tag = track.station(&refill.station).map(|s| s.tag.as_str());
    let at_bar = snapshot.last_tag_read.as_ref().map(|r| r.tag.as_str()) == bar_tag;
    if !at_bar {
        queue.set_phase(order.id, Phase::ToRefill);
        drive(service, track, &refill.station).await?;
    }

    // 2. one pour per glass, nudging the next glass under the spout between
    queue.set_phase(order.id, Phase::Fueling);
    for shot in 0..order.shots {
        if shot > 0 {
            nudge(service, track).await?;
        }
        pour(service, refill.shot_ml).await?;
        queue.update(order.id, |o| o.poured = shot + 1);
    }

    // 3. off to the guest
    queue.set_phase(order.id, Phase::Delivering);
    drive(service, track, &order.target).await
}

fn drive_command(track: &TrackConfig, target: &str) -> CommandKind {
    CommandKind::Drive {
        target: target.to_string(),
        max_speed: track.train.max_speed,
        start_speed: track.train.start_speed,
        accel: track.train.accel,
        max_run_ms_without_tag: track.train.watchdog_ms(),
        track_version: track.table().version,
        ttl_ms: track.train.default_ttl_ms,
    }
}

pub async fn drive(service: &TrainService, track: &TrackConfig, target: &str) -> Result<(), String> {
    // subscribe before sending so the terminal event cannot slip past
    let mut rx = service.subscribe();
    let cmd_id = service
        .send_command(drive_command(track, target))
        .map_err(|err| err.message())?;
    // no time limit: a ride takes as long as it takes; a click or any
    // manual control is what ends a stuck one
    match wait_train(&mut rx, service, cmd_id).await {
        ExecKind::Completed => Ok(()),
        ExecKind::Failed { error } => Err(format!("Fahrt abgebrochen: {}", error.reason)),
        other => Err(format!("Fahrt abgebrochen ({other:?})")),
    }
}

async fn nudge(service: &TrainService, track: &TrackConfig) -> Result<(), String> {
    let mut rx = service.subscribe();
    let cmd_id = service
        .send_command(CommandKind::Steps {
            steps: track.refill.steps,
            direction: Direction::Forward,
            speed: track.refill.speed,
            ttl_ms: track.train.default_ttl_ms,
        })
        .map_err(|err| err.message())?;
    let wait = Duration::from_millis(track.refill.nudge_wait_ms);
    let result = tokio::time::timeout(wait, wait_train(&mut rx, service, cmd_id)).await;
    // the firmware does not report `completed` for steps yet: on timeout,
    // assume the nudge is done and free the busy slot ourselves
    service.release_command(cmd_id);
    match result {
        Ok(ExecKind::Completed) | Err(_) => Ok(()),
        Ok(ExecKind::Failed { error }) => Err(format!("Rangieren fehlgeschlagen: {}", error.reason)),
        Ok(other) => Err(format!("Rangieren abgebrochen ({other:?})")),
    }
}

async fn pour(service: &TrainService, volume_ml: u32) -> Result<(), String> {
    let mut rx = service.subscribe();
    let cmd_id = service
        .send_fuel_command(FuelCommandKind::Dispense {
            volume_ml,
            ttl_ms: FUEL_TTL_MS,
        })
        .map_err(|err| err.message())?;
    match tokio::time::timeout(POUR_TIMEOUT, wait_fuel(&mut rx, service, cmd_id)).await {
        Ok(FuelEventKind::Completed) => Ok(()),
        Ok(FuelEventKind::Failed { error }) => Err(match error.code.as_str() {
            "TANK_EMPTY" => "Tank leer – bitte an der Bar nachfüllen.".into(),
            "NO_GLASS" => "Kein Glas unter dem Zapfhahn.".into(),
            _ => format!("Betankung fehlgeschlagen: {}", error.reason),
        }),
        Ok(other) => Err(format!("Betankung abgebrochen ({other:?})")),
        Err(_) => Err("Die Tankstelle hat sich nicht zurückgemeldet.".into()),
    }
}

/// Wait for the terminal exec event of `cmd_id`.
async fn wait_train(
    rx: &mut broadcast::Receiver<TrainServiceEvent>,
    service: &TrainService,
    cmd_id: Uuid,
) -> ExecKind {
    let find = |events: &[crate::types::ExecEvent]| {
        events
            .iter()
            .find(|e| e.envelope.cmd_id == CmdId::Id(cmd_id) && e.kind.is_terminal())
            .map(|e| e.kind.clone())
    };
    loop {
        match rx.recv().await {
            Ok(TrainServiceEvent::Events(events)) => {
                if let Some(kind) = find(&events) {
                    return kind;
                }
            }
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(_)) => {
                if let Some(kind) = find(&service.snapshot().events) {
                    return kind;
                }
            }
            Err(broadcast::error::RecvError::Closed) => std::future::pending().await,
        }
    }
}

/// Wait for the terminal fuel event of `cmd_id`.
async fn wait_fuel(
    rx: &mut broadcast::Receiver<TrainServiceEvent>,
    service: &TrainService,
    cmd_id: Uuid,
) -> FuelEventKind {
    let matches = |e: &crate::types::FuelEvent| {
        e.envelope.cmd_id == CmdId::Id(cmd_id) && e.kind.is_terminal()
    };
    loop {
        match rx.recv().await {
            Ok(TrainServiceEvent::FuelEvent(event)) if matches(&event) => return event.kind,
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(_)) => {
                if let Some(event) = service.snapshot().fuel_last_event
                    && matches(&event)
                {
                    return event.kind;
                }
            }
            Err(broadcast::error::RecvError::Closed) => std::future::pending().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_all_fails_open_orders_only() {
        let queue = OrderQueue::new();
        let done = queue.place("Tankstation", "Raucherecke", 1).id;
        queue.set_phase(done, Phase::Arrived);
        let running = queue.place("Tankstation", "Raucherecke", 2).id;
        queue.set_phase(running, Phase::Delivering);
        let waiting = queue.place("Tankstation", "Raucherecke", 3).id;

        assert_eq!(queue.cancel_all("Manuell gestoppt."), 2);
        assert_eq!(queue.get(done).unwrap().phase, Phase::Arrived);
        let running = queue.get(running).unwrap();
        assert_eq!(running.phase, Phase::Failed);
        assert_eq!(running.failed_in, Some(Phase::Delivering));
        assert_eq!(queue.get(waiting).unwrap().phase, Phase::Failed);
        assert!(queue.next_queued().is_none(), "nothing left to start");
    }
}
