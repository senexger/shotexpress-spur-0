use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, QoS};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::config::Config;
use crate::fuel_log::FuelLog;
use crate::types::{
    CmdId, CommandKind, CommandMessage, ConnectionState, ControllerSnapshot, Envelope, ExecEvent,
    ExecKind, FuelCommandKind, FuelCommandMessage, FuelEvent, FuelStatusMessage, ManualAction,
    RawStatus, StatusMessage, TOPIC_COMMAND, TOPIC_EVENT_EXEC, TOPIC_EVENT_TAG,
    TOPIC_FUEL_COMMAND, TOPIC_FUEL_EVENT, TOPIC_FUEL_SENSOR, TOPIC_FUEL_STATUS, TOPIC_ACCEL, TOPIC_SPEED, TOPIC_STATUS,
    TOPIC_STEPS, TOPIC_TRACK, TagRead, TrackTable, TrainServiceEvent, now_ms,
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
    /// Station uid hidden from the train's tag table: a ride started on its
    /// own target tag, and must read another tag before it may stop there.
    hidden_uid: Option<String>,
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
    /// Lock order: `state` before `fuel_log`, never the other way round.
    fuel_log: Mutex<FuelLog>,
    /// Tag table from track.toml, (re)published retained on every connect.
    track_table: Mutex<Option<TrackTable>>,
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
                hidden_uid: None,
                last_raw_status: None,
                train_last_seen_ms: None,
                train_online: false,
                speed_hz: None,
                accel: None,
                fuel_last_event: None,
                fuel_status: None,
                fuel_distance_mm: None,
            }),
            fuel_log: Mutex::new(FuelLog::open(&cfg.fuel_log_dir)),
            track_table: Mutex::new(None),
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

    /// Remember the tag table and publish it (retained) to the train.
    pub fn set_track_table(&self, table: TrackTable) {
        *self.track_table.lock().expect("track table lock poisoned") = Some(table);
        let state = self.lock();
        self.publish_track_table(&state);
    }

    /// Lock order: `state` before `track_table`, so callers pass the state in.
    fn publish_track_table(&self, state: &ServiceState) -> Option<u32> {
        let table = self.track_table.lock().expect("track table lock poisoned").clone()?;
        let table = hide_uid(table, state.hidden_uid.as_deref());
        let version = table.version;
        let payload = serde_json::to_vec(&table).expect("track table serializes");
        // retained: the train gets it right after every (re)connect; while
        // the broker is unreachable this fails and ConnAck publishes again
        match self.client.try_publish(TOPIC_TRACK, QoS::AtLeastOnce, true, payload) {
            Ok(()) => tracing::info!(version = table.version, tags = table.tags.len(), "published tag table"),
            Err(err) => tracing::warn!(%err, "failed to publish tag table"),
        }
        Some(version)
    }

    /// Show a hidden station tag to the train again.
    fn unhide_uid(&self, state: &mut ServiceState) {
        if let Some(uid) = state.hidden_uid.take() {
            tracing::info!(%uid, "station tag armed again");
            self.publish_track_table(state);
        }
    }

    /// A ride to the station the train stands on would stop at once (the
    /// tag is read again when the train starts). Hide that tag until the
    /// train has read another one. Returns the table version to ride with.
    fn prepare_ride(&self, state: &mut ServiceState, target: &str, version: u32) -> u32 {
        let target_uid = self
            .track_table
            .lock()
            .expect("track table lock poisoned")
            .as_ref()
            .and_then(|t| t.station_uid(target).map(str::to_string));
        let on_target = match (&target_uid, &state.last_tag_read) {
            (Some(uid), Some(read)) => &read.tag == uid,
            _ => false,
        };
        let wanted = if on_target { target_uid } else { None };
        if state.hidden_uid == wanted && wanted.is_none() {
            return version; // the real table is already out
        }
        if let Some(uid) = &wanted {
            tracing::info!(%uid, target, "train stands on its target tag; hidden until another tag is read");
        }
        state.hidden_uid = wanted;
        self.publish_track_table(state).unwrap_or(version)
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
        let volume_ml = match kind {
            FuelCommandKind::Dispense { volume_ml, .. } => Some(volume_ml),
            FuelCommandKind::Stop => None,
        };
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
        drop(state);
        let mut log = self.fuel_log();
        match volume_ml {
            Some(volume_ml) => log.dispense_sent(cmd_id, volume_ml),
            None => log.stop_sent(cmd_id),
        }
        Ok(cmd_id)
    }

    /// Publish an acked command (drive / steps / stop) with a fresh cmd_id
    /// and take the busy slot until its terminal exec event arrives.
    pub fn send_command(&self, mut kind: CommandKind) -> Result<Uuid, CommandError> {
        let mut state = self.lock();
        if state.is_busy {
            return Err(CommandError::Busy);
        }
        if state.connection != ConnectionState::Connected {
            return Err(CommandError::NotConnected);
        }
        match &mut kind {
            CommandKind::Drive {
                target,
                track_version,
                ..
            } => {
                // published before the command, same client, QoS 1: the
                // train has the table before the ride starts
                *track_version = self.prepare_ride(&mut state, target, *track_version);
            }
            _ => self.unhide_uid(&mut state),
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

    /// Drop whatever ride is in flight from the server's view, so a new
    /// one can be sent at once (the train cancels its old ride itself).
    pub fn release_current(&self) {
        let mut state = self.lock();
        state.current_command_id = None;
        self.set_busy(&mut state, false);
    }

    /// Free the busy slot held by `cmd_id`, for commands whose terminal
    /// event never comes (a `steps` nudge, or a train that went silent).
    pub fn release_command(&self, cmd_id: Uuid) {
        let mut state = self.lock();
        if state.current_command_id == Some(cmd_id) {
            state.current_command_id = None;
            self.set_busy(&mut state, false);
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
        // manual override ends any ride
        self.unhide_uid(&mut state);
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, ServiceState> {
        self.state.lock().expect("train service state poisoned")
    }

    fn fuel_log(&self) -> MutexGuard<'_, FuelLog> {
        self.fuel_log.lock().expect("fuel log poisoned")
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
                self.fuel_log().tag(&read.tag);
                let mut state = self.lock();
                if state.hidden_uid.as_ref().is_some_and(|uid| *uid != read.tag) {
                    self.unhide_uid(&mut state);
                }
                state.last_tag_read = Some(read.clone());
                let _ = self.tx.send(TrainServiceEvent::TagRead(read));
            }
            TOPIC_FUEL_EVENT => match serde_json::from_slice::<FuelEvent>(payload) {
                Ok(event) => {
                    self.fuel_log().fuel_event(&event);
                    self.lock().fuel_last_event = Some(event.clone());
                    let _ = self.tx.send(TrainServiceEvent::FuelEvent(event));
                }
                Err(err) => {
                    self.fuel_log().parse_error(TOPIC_FUEL_EVENT, payload);
                    tracing::warn!(%err, payload = %String::from_utf8_lossy(payload), "failed to parse fuel event");
                }
            },
            TOPIC_FUEL_STATUS => match serde_json::from_slice::<FuelStatusMessage>(payload) {
                Ok(status) => {
                    self.fuel_log().fuel_status(&status);
                    self.lock().fuel_status = Some(status.clone());
                    let _ = self.tx.send(TrainServiceEvent::FuelStatus(status));
                }
                Err(err) => {
                    self.fuel_log().parse_error(TOPIC_FUEL_STATUS, payload);
                    tracing::warn!(%err, payload = %String::from_utf8_lossy(payload), "failed to parse fuel status");
                }
            },
            TOPIC_FUEL_SENSOR => {
                let raw = String::from_utf8_lossy(payload);
                let Ok(mm) = raw.trim().parse::<i32>() else {
                    self.fuel_log().parse_error(TOPIC_FUEL_SENSOR, payload);
                    tracing::warn!(payload = %raw, "ignoring non-integer fuel sensor payload");
                    return;
                };
                self.fuel_log().sensor(mm);
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
            self.unhide_uid(&mut state);
        }
    }

    fn update_connection(&self, connection: ConnectionState) {
        let mut state = self.lock();
        if state.connection == connection {
            return;
        }
        let was_connected = state.connection == ConnectionState::Connected;
        let is_connected = connection == ConnectionState::Connected;
        if was_connected != is_connected {
            self.fuel_log().mqtt(is_connected);
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

/// The table as the train gets it: `hidden` gets a uid no tag has, and
/// the version changes with it so a ride can name the table it expects.
fn hide_uid(mut table: TrackTable, hidden: Option<&str>) -> TrackTable {
    let Some(hidden) = hidden else {
        return table;
    };
    for rule in table.tags.iter_mut().filter(|r| r.uid == hidden) {
        rule.uid = "-".to_string();
    }
    table.version ^= 0x5a5a_5a5a;
    table
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
                service.publish_track_table(&service.lock());
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::types::{TagKind, TagRule};

    /// Pretend the train finished its current ride. Returns false if no
    /// ride is in flight.
    pub(crate) fn complete_current(service: &TrainService) -> bool {
        let Some(cmd_id) = service.lock().current_command_id else {
            return false;
        };
        service.handle_exec_event(ExecEvent {
            envelope: Envelope {
                msg_id: Uuid::new_v4(),
                cmd_id: CmdId::Id(cmd_id),
                seq: 1,
                ts_ms: now_ms(),
            },
            kind: ExecKind::Completed,
        });
        true
    }

    const HOME: &str = "041A45D2462490";
    const OTHER: &str = "04402852803C80";

    /// The event loop is returned only to keep the channel open.
    pub(crate) fn service() -> (Arc<TrainService>, EventLoop) {
        let cfg = Config {
            mqtt_host: "localhost".into(),
            mqtt_port: 1883,
            mqtt_username: None,
            mqtt_password: None,
            mqtt_client_id: "test".into(),
            http_port: 0,
            public_port: None,
            mdns_hostname: None,
            track_path: "track.toml".into(),
            fuel_log_dir: std::env::temp_dir().join("shotexpress-test-logs"),
        };
        // the event loop is never polled: publishes just queue up
        let (service, eventloop) = TrainService::new(&cfg);
        service.update_connection(ConnectionState::Connected);
        service.set_track_table(TrackTable {
            version: 7,
            tags: vec![TagRule {
                uid: HOME.into(),
                kind: TagKind::Station {
                    station: "Raucherecke".into(),
                },
            }],
        });
        (service, eventloop)
    }

    fn ride(service: &TrainService) {
        service
            .send_command(CommandKind::Drive {
                target: "Raucherecke".into(),
                max_speed: 1.0,
                start_speed: 0.3,
                accel: 0.5,
                max_run_ms_without_tag: 30_000,
                track_version: 7,
                ttl_ms: 60_000,
            })
            .unwrap();
    }

    fn hidden(service: &TrainService) -> Option<String> {
        service.lock().hidden_uid.clone()
    }

    #[test]
    fn ride_from_its_own_target_tag_waits_for_another_tag() {
        let (service, _eventloop) = service();
        service.handle_publish(TOPIC_EVENT_TAG, HOME.as_bytes());
        ride(&service);
        assert_eq!(hidden(&service).as_deref(), Some(HOME));

        // re-reading the same tag while driving off keeps it hidden
        service.handle_publish(TOPIC_EVENT_TAG, HOME.as_bytes());
        assert_eq!(hidden(&service).as_deref(), Some(HOME));

        // any other tag arms the stop again
        service.handle_publish(TOPIC_EVENT_TAG, OTHER.as_bytes());
        assert_eq!(hidden(&service), None);
    }

    #[test]
    fn ride_from_elsewhere_uses_the_real_table() {
        let (service, _eventloop) = service();
        service.handle_publish(TOPIC_EVENT_TAG, OTHER.as_bytes());
        ride(&service);
        assert_eq!(hidden(&service), None);
    }

    #[test]
    fn manual_stop_shows_the_tag_again() {
        let (service, _eventloop) = service();
        service.handle_publish(TOPIC_EVENT_TAG, HOME.as_bytes());
        ride(&service);
        service.send_manual_command(ManualAction::Stop).unwrap();
        assert_eq!(hidden(&service), None);
    }

    #[test]
    fn hide_uid_changes_uid_and_version() {
        let table = TrackTable {
            version: 7,
            tags: vec![TagRule {
                uid: HOME.into(),
                kind: TagKind::Speed { speed: 0.5 },
            }],
        };
        let masked = hide_uid(table.clone(), Some(HOME));
        assert_eq!(masked.tags[0].uid, "-");
        assert_ne!(masked.version, 7);
        assert_eq!(hide_uid(table, None).version, 7);
    }
}
