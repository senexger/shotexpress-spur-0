use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, QoS};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::config::Config;
use crate::types::{
    CmdId, CommandKind, CommandMessage, ConnectionState, ControllerSnapshot, Envelope, ExecEvent,
    ManualAction, MoveCommandParameters, StatusMessage, TOPIC_COMMAND, TOPIC_EVENT_EXEC,
    TOPIC_STATUS, TrainServiceEvent,
};

const BROADCAST_CAPACITY: usize = 64;
const RECONNECT_PERIOD: Duration = Duration::from_secs(2);

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
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TrainServiceEvent> {
        self.tx.subscribe()
    }

    pub fn send_move_command(
        &self,
        parameters: MoveCommandParameters,
        ttl_ms: u64,
    ) -> Result<(), CommandError> {
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
            kind: CommandKind::MoveTo {
                params: parameters,
                ttl_ms,
            },
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
                Ok(())
            }
            Err(err) => {
                let reason = err.to_string();
                self.set_error(&mut state, Some(reason.clone()));
                self.set_busy(&mut state, false);
                Err(CommandError::Publish(reason))
            }
        }
    }

    pub fn send_manual_command(&self, action: ManualAction) -> Result<(), CommandError> {
        let state = self.lock();
        if state.connection != ConnectionState::Connected {
            return Err(CommandError::NotConnected);
        }
        self.client
            .try_publish(
                TOPIC_COMMAND,
                QoS::AtLeastOnce,
                false,
                action.firmware_payload(),
            )
            .map_err(|err| CommandError::Publish(err.to_string()))
    }

    fn lock(&self) -> MutexGuard<'_, ServiceState> {
        self.state.lock().expect("train service state poisoned")
    }

    fn handle_publish(&self, topic: &str, payload: &[u8]) {
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
                Err(err) => {
                    tracing::warn!(%err, payload = %String::from_utf8_lossy(payload), "failed to parse status payload");
                }
            },
            TOPIC_EVENT_EXEC => match serde_json::from_slice::<ExecEvent>(payload) {
                Ok(event) => self.handle_exec_event(event),
                Err(err) => {
                    tracing::warn!(%err, payload = %String::from_utf8_lossy(payload), "failed to parse exec event payload");
                }
            },
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

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_millis() as u64
}
