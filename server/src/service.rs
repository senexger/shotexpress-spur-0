use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, QoS};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::config::Config;
use crate::types::{
    CmdId, CommandKind, CommandMessage, ConnectionState, ControllerSnapshot, Envelope, ExecEvent,
    ExecKind, FuelCommandKind, FuelCommandMessage, FuelEvent, FuelStatusMessage, ManualAction,
    RawStatus, StatusMessage, TOPIC_COMMAND, TOPIC_EVENT_EXEC, TOPIC_EVENT_TAG,
    TOPIC_FUEL_COMMAND, TOPIC_FUEL_EVENT, TOPIC_FUEL_SENSOR, TOPIC_FUEL_STATUS, TOPIC_ACCEL, TOPIC_SPEED, TOPIC_STATUS,
    TOPIC_STEPS, TagRead, TrainServiceEvent, now_ms,
};

const BROADCAST_CAPACITY: usize = 64;
const RECONNECT_PERIOD: Duration = Duration::from_secs(2);
/// The firmware heartbeats every 1 s (MQTT_PUBLISH_INTERVAL in Train.ino);
/// 10 missed heartbeats means the train is gone.
const TRAIN_OFFLINE_AFTER: Duration = Duration::from_secs(10);
const LIVENESS_SWEEP_PERIOD: Duration = Duration::from_secs(1);
/// Matches MAX_SPEED_HZ in Train.ino (the firmware clamps too).
pub const MAX_SPEED_HZ: u32 = 20_000;
/// Sanity cap for one manual move, either direction.
pub const MAX_MOVE_STEPS: i32 = 1_000_000;
/// Sanity cap for the start acceleration (steps/s²).
pub const MAX_ACCEL: u32 = 1_000_000;
/// Sanity cap for one pour; the station's own pour-time cap is stricter.
pub const MAX_FILL_ML: u32 = 500;
/// A pour command older than this is dropped by the station (`expired`).
pub const FUEL_TTL_MS: u64 = 15_000;

#[derive(Debug)]
pub enum CommandError {
    Busy,
    NotConnected,
    Publish(String),
}

impl CommandError {
    pub fn message(&self) -> String {
        match self {
            CommandError::Busy => "Train is busy processing another command".to_string(),
            CommandError::NotConnected => "MQTT connection not ready".to_string(),
            CommandError::Publish(reason) => reason.clone(),
        }
    }
}

struct ServiceState {
    connection: ConnectionState,
    last_status: Option<StatusMessage>,
    events: Vec<ExecEvent>,
    is_busy: bool,
    error: Option<String>,
    current_command_id: Option<Uuid>,
    seen_exec_msg_ids: HashSet<Uuid>,
    seq: u64,
    /// Last two progress tags, oldest first — two consecutive reads pin the
    /// train to a unique ring node (track.toml pairs are validated unique).
    last_tags: (Option<String>, Option<String>),
    /// Last raw read from the tag topic (setup aid, not localization).
    last_tag_read: Option<TagRead>,
    /// Last non-JSON status payload (legacy firmware "Alive" heartbeat).
    last_raw_status: Option<RawStatus>,
    /// Receive time of the last message from the train on any topic.
    train_last_seen_ms: Option<u64>,
    /// False until the first train message, and again once
    /// TRAIN_OFFLINE_AFTER passes without one.
    train_online: bool,
    /// Last value seen on the retained speed topic.
    speed_hz: Option<u32>,
    accel: Option<u32>,
    fuel_last_event: Option<FuelEvent>,
    fuel_status: Option<FuelStatusMessage>,
    fuel_distance_mm: Option<i32>,
}

pub struct TrainService {
    client: AsyncClient,
    state: Mutex<ServiceState>,
    tx: broadcast::Sender<TrainServiceEvent>,
}

impl TrainService {
    pub fn new(cfg: &Config) -> (Arc<Self>, EventLoop) {
        let mut options = MqttOptions::new(
            cfg.mqtt_client_id.clone(),
            cfg.mqtt_host.clone(),
            cfg.mqtt_port,
        );
        options.set_keep_alive(Duration::from_secs(60));
        options.set_clean_session(true);
        if let (Some(username), Some(password)) = (&cfg.mqtt_username, &cfg.mqtt_password) {
            options.set_credentials(username.clone(), password.clone());
        }

        let (client, eventloop) = AsyncClient::new(options, 16);
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);

        let service = Arc::new(TrainService {
            client,
            state: Mutex::new(ServiceState {
                connection: ConnectionState::Connecting,
                last_status: None,
                events: Vec::new(),
                is_busy: false,
                error: None,
                current_command_id: None,
                seen_exec_msg_ids: HashSet::new(),
                seq: 0,
                last_tags: (None, None),
                last_tag_read: None,
                last_raw_status: None,
                train_last_seen_ms: None,
                train_online: false,
                speed_hz: None,
                accel: None,
                fuel_last_event: None,
                fuel_status: None,
                fuel_distance_mm: None,
            }),
            tx,
        });

        (service, eventloop)
    }

    pub fn snapshot(&self) -> ControllerSnapshot {
        let state = self.lock();
        Self::snapshot_locked(&state)
    }

    fn snapshot_locked(state: &ServiceState) -> ControllerSnapshot {
        ControllerSnapshot {
            connection: state.connection,
            last_status: state.last_status.clone(),
            events: state.events.clone(),
            is_busy: state.is_busy,
            error: state.error.clone(),
            last_known_tag: state.last_tags.1.clone(),
            last_tag_read: state.last_tag_read.clone(),
            last_raw_status: state.last_raw_status.clone(),
            train_online: state.train_online,
            speed_hz: state.speed_hz,
            accel: state.accel,
            fuel_last_event: state.fuel_last_event.clone(),
            fuel_status: state.fuel_status.clone(),
            fuel_distance_mm: state.fuel_distance_mm,
        }
    }

    /// Any message from the train counts as a sign of life.
    fn note_train_seen(&self, state: &mut ServiceState) {
        state.train_last_seen_ms = Some(now_ms());
        if !state.train_online {
            state.train_online = true;
            let _ = self.tx.send(TrainServiceEvent::Liveness(true));
        }
    }

    /// Flip the train to dead if the heartbeat has been quiet too long.
    fn check_liveness(&self) {
        let mut state = self.lock();
        if !state.train_online {
            return;
        }
        let stale = match state.train_last_seen_ms {
            Some(last_seen) => now_ms().saturating_sub(last_seen) > TRAIN_OFFLINE_AFTER.as_millis() as u64,
            None => true,
        };
        if stale {
            state.train_online = false;
            let _ = self.tx.send(TrainServiceEvent::Liveness(false));
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TrainServiceEvent> {
        self.tx.subscribe()
    }

    /// The last two progress tags (oldest first), for pinning the ring node.
    pub fn last_tag_pair(&self) -> (Option<String>, Option<String>) {
        self.lock().last_tags.clone()
    }

    /// Publish a fuel station command (dispense / stop) with a fresh cmd_id.
    /// Test setup: no busy interlock with the train yet; the UI just shows
    /// whatever the station reports back.
    pub fn send_fuel_command(&self, kind: FuelCommandKind) -> Result<Uuid, CommandError> {
        let mut state = self.lock();
        if state.connection != ConnectionState::Connected {
            return Err(CommandError::NotConnected);
        }
        let cmd_id = Uuid::new_v4();
        state.seq += 1;
        let command = FuelCommandMessage {
            envelope: Envelope {
                msg_id: Uuid::new_v4(),
                cmd_id: CmdId::Id(cmd_id),
                seq: state.seq,
                ts_ms: now_ms(),
            },
            kind,
        };
        let payload =
            serde_json::to_string(&command).expect("FuelCommandMessage serialization cannot fail");
        self.client
            .try_publish(TOPIC_FUEL_COMMAND, QoS::AtLeastOnce, false, payload)
            .map_err(|err| CommandError::Publish(err.to_string()))?;
        Ok(cmd_id)
    }

    /// Publish an acked command (drive / steps / stop) with a fresh cmd_id
    /// and take the busy slot until its terminal exec event arrives.
    pub fn send_command(&self, kind: CommandKind) -> Result<Uuid, CommandError> {
        let mut state = self.lock();
        if state.is_busy {
            return Err(CommandError::Busy);
        }
        if state.connection != ConnectionState::Connected {
            return Err(CommandError::NotConnected);
        }

        let cmd_id = Uuid::new_v4();
        state.seq += 1;
        let command = CommandMessage {
            envelope: Envelope {
                msg_id: Uuid::new_v4(),
                cmd_id: CmdId::Id(cmd_id),
                seq: state.seq,
                ts_ms: now_ms(),
            },
            kind,
        };
        let payload =
            serde_json::to_string(&command).expect("CommandMessage serialization cannot fail");

        match self
            .client
            .try_publish(TOPIC_COMMAND, QoS::AtLeastOnce, false, payload)
        {
            Ok(()) => {
                state.current_command_id = Some(cmd_id);
                state.events.clear();
                state.seen_exec_msg_ids.clear();
                self.emit_events(&state);
                self.set_busy(&mut state, true);
                self.set_error(&mut state, None);
                Ok(cmd_id)
            }
            Err(err) => {
                let reason = err.to_string();
                self.set_error(&mut state, Some(reason.clone()));
                self.set_busy(&mut state, false);
                Err(CommandError::Publish(reason))
            }
        }
    }

    /// Publish a raw firmware command. Manual commands override whatever the
    /// train is doing, so any in-flight acked command is abandoned: the busy
    /// slot frees up immediately instead of waiting for a terminal exec event
    /// that the overridden command may never produce.
    pub fn send_manual_command(&self, action: ManualAction) -> Result<(), CommandError> {
        self.publish_raw(&[(TOPIC_COMMAND, action.firmware_payload().to_string(), false)])
    }

    /// Set the max speed (retained, raw steps/s) and drive forward with it.
    /// Test setup: speed comes from the UI, not track.toml.
    /// Optionally sets the acceleration first.
    pub fn send_speed(&self, hz: u32, accel: Option<u32>) -> Result<(), CommandError> {
        let mut messages = settings_messages(Some(hz), accel);
        messages.push((TOPIC_COMMAND, ManualAction::Forward.firmware_payload().to_string(), false));
        self.publish_raw(&messages)
    }

    /// Move a relative number of steps (negative = backward), optionally
    /// setting the speed and acceleration first.
    pub fn send_move(
        &self,
        steps: i32,
        hz: Option<u32>,
        accel: Option<u32>,
    ) -> Result<(), CommandError> {
        let mut messages = settings_messages(hz, accel);
        messages.push((TOPIC_STEPS, steps.to_string(), false));
        self.publish_raw(&messages)
    }

    /// Set the start acceleration (retained) without moving the train.
    pub fn send_accel(&self, accel: u32) -> Result<(), CommandError> {
        self.publish_raw(&settings_messages(None, Some(accel)))
    }

    /// Publish raw firmware messages in order. Like every manual command
    /// this abandons any in-flight acked command (see send_manual_command).
    fn publish_raw(&self, messages: &[(&str, String, bool)]) -> Result<(), CommandError> {
        let mut state = self.lock();
        if state.connection != ConnectionState::Connected {
            return Err(CommandError::NotConnected);
        }
        for (topic, payload, retain) in messages {
            self.client
                .try_publish(*topic, QoS::AtLeastOnce, *retain, payload.clone())
                .map_err(|err| CommandError::Publish(err.to_string()))?;
        }

        state.current_command_id = None;
        self.set_busy(&mut state, false);
        self.set_error(&mut state, None);
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, ServiceState> {
        self.state.lock().expect("train service state poisoned")
    }

    fn handle_publish(&self, topic: &str, payload: &[u8]) {
        if matches!(topic, TOPIC_STATUS | TOPIC_EVENT_EXEC | TOPIC_EVENT_TAG) {
            let mut state = self.lock();
            self.note_train_seen(&mut state);
        }
        match topic {
            TOPIC_STATUS => match serde_json::from_slice::<StatusMessage>(payload) {
                Ok(status) if status.battery_pct <= 100 => {
                    let mut state = self.lock();
                    state.last_status = Some(status.clone());
                    let _ = self.tx.send(TrainServiceEvent::Status(status));
                }
                Ok(status) => {
                    tracing::warn!(
                        battery_pct = status.battery_pct,
                        "dropping status with out-of-range battery_pct"
                    );
                }
                Err(_) => {
                    // legacy firmware heartbeats plain "Alive" — record it as
                    // a raw status so the UI can at least show the train online
                    let raw = String::from_utf8_lossy(payload).trim().to_string();
                    if raw.is_empty() {
                        return;
                    }
                    let status = RawStatus {
                        raw,
                        ts_ms: now_ms(),
                    };
                    let mut state = self.lock();
                    state.last_raw_status = Some(status.clone());
                    let _ = self.tx.send(TrainServiceEvent::RawStatus(status));
                }
            },
            TOPIC_EVENT_EXEC => match serde_json::from_slice::<ExecEvent>(payload) {
                Ok(event) => self.handle_exec_event(event),
                Err(err) => {
                    tracing::warn!(%err, payload = %String::from_utf8_lossy(payload), "failed to parse exec event payload");
                }
            },
            TOPIC_EVENT_TAG => {
                // raw uid string, no envelope — record verbatim with our clock
                let tag = String::from_utf8_lossy(payload).trim().to_string();
                if tag.is_empty() {
                    return;
                }
                let read = TagRead {
                    tag,
                    ts_ms: now_ms(),
                };
                let mut state = self.lock();
                state.last_tag_read = Some(read.clone());
                let _ = self.tx.send(TrainServiceEvent::TagRead(read));
            }
            TOPIC_FUEL_EVENT => match serde_json::from_slice::<FuelEvent>(payload) {
                Ok(event) => {
                    self.lock().fuel_last_event = Some(event.clone());
                    let _ = self.tx.send(TrainServiceEvent::FuelEvent(event));
                }
                Err(err) => {
                    tracing::warn!(%err, payload = %String::from_utf8_lossy(payload), "failed to parse fuel event");
                }
            },
            TOPIC_FUEL_STATUS => match serde_json::from_slice::<FuelStatusMessage>(payload) {
                Ok(status) => {
                    self.lock().fuel_status = Some(status.clone());
                    let _ = self.tx.send(TrainServiceEvent::FuelStatus(status));
                }
                Err(err) => {
                    tracing::warn!(%err, payload = %String::from_utf8_lossy(payload), "failed to parse fuel status");
                }
            },
            TOPIC_FUEL_SENSOR => {
                let raw = String::from_utf8_lossy(payload);
                let Ok(mm) = raw.trim().parse::<i32>() else {
                    tracing::warn!(payload = %raw, "ignoring non-integer fuel sensor payload");
                    return;
                };
                self.lock().fuel_distance_mm = Some(mm);
                let _ = self.tx.send(TrainServiceEvent::FuelDistance(mm));
            }
            TOPIC_ACCEL => {
                let raw = String::from_utf8_lossy(payload);
                let Ok(accel) = raw.trim().parse::<u32>() else {
                    tracing::warn!(payload = %raw, "ignoring non-integer accel payload");
                    return;
                };
                self.lock().accel = Some(accel);
                let _ = self.tx.send(TrainServiceEvent::Accel(accel));
            }
            TOPIC_SPEED => {
                let raw = String::from_utf8_lossy(payload);
                let Ok(hz) = raw.trim().parse::<u32>() else {
                    tracing::warn!(payload = %raw, "ignoring non-integer speed payload");
                    return;
                };
                self.lock().speed_hz = Some(hz);
                let _ = self.tx.send(TrainServiceEvent::Speed(hz));
            }
            other => {
                tracing::debug!(topic = other, "ignoring message on unexpected topic");
            }
        }
    }

    fn handle_exec_event(&self, event: ExecEvent) {
        let mut state = self.lock();
        if !state.seen_exec_msg_ids.insert(event.envelope.msg_id) {
            return;
        }

        // position tracking listens to every progress event, even from
        // commands this server didn't send (e.g. a second admin instance)
        if let ExecKind::Progress {
            last_tag, skipped, ..
        } = &event.kind
        {
            if *skipped > 0 {
                // the missed reads break the consecutive-pair chain
                state.last_tags = (None, Some(last_tag.clone()));
            } else {
                state.last_tags = (state.last_tags.1.take(), Some(last_tag.clone()));
            }
        }

        if let Some(current) = state.current_command_id
            && event.envelope.cmd_id != CmdId::Id(current)
        {
            return;
        }

        let terminal = event.kind.is_terminal();
        state.events.push(event);
        self.emit_events(&state);

        if terminal {
            state.current_command_id = None;
            self.set_busy(&mut state, false);
        }
    }

    fn update_connection(&self, connection: ConnectionState) {
        let mut state = self.lock();
        if state.connection == connection {
            return;
        }
        state.connection = connection;
        let _ = self.tx.send(TrainServiceEvent::Connection(connection));
    }

    fn emit_events(&self, state: &ServiceState) {
        let _ = self
            .tx
            .send(TrainServiceEvent::Events(state.events.clone()));
    }

    fn set_busy(&self, state: &mut ServiceState, is_busy: bool) {
        if state.is_busy == is_busy {
            return;
        }
        state.is_busy = is_busy;
        let _ = self.tx.send(TrainServiceEvent::Busy(is_busy));
    }

    fn set_error(&self, state: &mut ServiceState, message: Option<String>) {
        if state.error == message {
            return;
        }
        state.error = message.clone();
        let _ = self.tx.send(TrainServiceEvent::Error(message));
    }

    fn set_error_standalone(&self, message: Option<String>) {
        let mut state = self.lock();
        self.set_error(&mut state, message);
    }
}

/// Retained speed/accel settings, published before a raw command so the
/// train applies them first.
fn settings_messages(hz: Option<u32>, accel: Option<u32>) -> Vec<(&'static str, String, bool)> {
    let mut messages = Vec::new();
    if let Some(accel) = accel {
        messages.push((TOPIC_ACCEL, accel.to_string(), true));
    }
    if let Some(hz) = hz {
        messages.push((TOPIC_SPEED, hz.to_string(), true));
    }
    messages
}

pub async fn run_liveness_sweep(service: Arc<TrainService>) {
    let mut interval = tokio::time::interval(LIVENESS_SWEEP_PERIOD);
    loop {
        interval.tick().await;
        service.check_liveness();
    }
}

pub async fn run_event_loop(service: Arc<TrainService>, mut eventloop: EventLoop) {
    loop {
        match eventloop.poll().await {
            Ok(Event::Incoming(Packet::ConnAck(_))) => {
                tracing::info!("connected to MQTT broker");
                // clean_session(true) drops subscriptions on disconnect; resubscribe on every ConnAck
                if let Err(err) = service.client.try_subscribe(TOPIC_STATUS, QoS::AtMostOnce) {
                    tracing::error!(%err, "failed to subscribe to status topic");
                }
                if let Err(err) = service
                    .client
                    .try_subscribe(TOPIC_EVENT_EXEC, QoS::AtLeastOnce)
                {
                    tracing::error!(%err, "failed to subscribe to exec event topic");
                }
                if let Err(err) = service
                    .client
                    .try_subscribe(TOPIC_EVENT_TAG, QoS::AtMostOnce)
                {
                    tracing::error!(%err, "failed to subscribe to tag event topic");
                }
                for (topic, qos) in [
                    (TOPIC_SPEED, QoS::AtMostOnce),
                    (TOPIC_ACCEL, QoS::AtMostOnce),
                    (TOPIC_FUEL_EVENT, QoS::AtLeastOnce),
                    (TOPIC_FUEL_STATUS, QoS::AtMostOnce),
                    (TOPIC_FUEL_SENSOR, QoS::AtMostOnce),
                ] {
                    if let Err(err) = service.client.try_subscribe(topic, qos) {
                        tracing::error!(%err, topic, "failed to subscribe");
                    }
                }
                service.update_connection(ConnectionState::Connected);
                service.set_error_standalone(None);
            }
            Ok(Event::Incoming(Packet::Publish(publish))) => {
                service.handle_publish(&publish.topic, &publish.payload);
            }
            Ok(Event::Incoming(Packet::Disconnect)) => {
                service.update_connection(ConnectionState::Disconnected);
            }
            Ok(_) => {}
            Err(err) => {
                tracing::warn!(%err, "MQTT connection error, retrying in {:?}", RECONNECT_PERIOD);
                service.update_connection(ConnectionState::Error);
                service.set_error_standalone(Some(err.to_string()));
                tokio::time::sleep(RECONNECT_PERIOD).await;
                service.update_connection(ConnectionState::Reconnecting);
            }
        }
    }
}
