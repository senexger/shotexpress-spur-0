pub use shotexpress_protocol::*;

use serde::{Deserialize, Serialize};

// ---- server-side view types (controllerTypes.ts) ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionState {
    Connecting,
    Connected,
    Reconnecting,
    Disconnected,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct ControllerSnapshot {
    pub connection: ConnectionState,
    #[serde(rename = "lastStatus")]
    pub last_status: Option<StatusMessage>,
    pub events: Vec<ExecEvent>,
    #[serde(rename = "isBusy")]
    pub is_busy: bool,
    pub error: Option<String>,
}

// ---- SSE / broadcast events (TrainServiceEvent) ----

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "payload", rename_all = "lowercase")]
pub enum TrainServiceEvent {
    Connection(ConnectionState),
    Status(StatusMessage),
    Events(Vec<ExecEvent>),
    Busy(bool),
    Error(Option<String>),
    Snapshot(ControllerSnapshot),
}

// ---- HTTP request bodies ----

#[derive(Debug, Default, Deserialize)]
pub struct CommandRequestBody {
    pub ttl_ms: Option<u64>,
    pub parameters: Option<PartialMoveParameters>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PartialMoveParameters {
    pub target: Option<Target>,
    pub speed: Option<f64>,
    pub direction: Option<Direction>,
    pub expected_tags: Option<Vec<String>>,
    pub stop_on_tag: Option<String>,
    pub max_run_ms_without_tag: Option<u64>,
}

impl PartialMoveParameters {
    pub fn merge_over(self, defaults: MoveCommandParameters) -> MoveCommandParameters {
        MoveCommandParameters {
            target: self.target.unwrap_or(defaults.target),
            speed: self.speed.unwrap_or(defaults.speed),
            direction: self.direction.unwrap_or(defaults.direction),
            expected_tags: self.expected_tags.unwrap_or(defaults.expected_tags),
            stop_on_tag: self.stop_on_tag.unwrap_or(defaults.stop_on_tag),
            max_run_ms_without_tag: self
                .max_run_ms_without_tag
                .unwrap_or(defaults.max_run_ms_without_tag),
        }
    }
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum ManualAction {
    Stop,
    Steps,
    Run,
}

impl ManualAction {
    /// Raw integer protocol of the current Train.ino firmware:
    /// 0 = stopMove(), 1 = move(1000 steps), 2 = runBackward() (continuous)
    pub fn firmware_payload(self) -> &'static str {
        match self {
            ManualAction::Stop => "0",
            ManualAction::Steps => "1",
            ManualAction::Run => "2",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn train_service_event_shapes() {
        let cases: Vec<(TrainServiceEvent, Value)> = vec![
            (
                TrainServiceEvent::Connection(ConnectionState::Reconnecting),
                json!({"type": "connection", "payload": "reconnecting"}),
            ),
            (
                TrainServiceEvent::Busy(true),
                json!({"type": "busy", "payload": true}),
            ),
            (
                TrainServiceEvent::Error(None),
                json!({"type": "error", "payload": null}),
            ),
            (
                TrainServiceEvent::Error(Some("boom".into())),
                json!({"type": "error", "payload": "boom"}),
            ),
            (
                TrainServiceEvent::Events(vec![]),
                json!({"type": "events", "payload": []}),
            ),
            (
                TrainServiceEvent::Snapshot(ControllerSnapshot {
                    connection: ConnectionState::Connecting,
                    last_status: None,
                    events: vec![],
                    is_busy: false,
                    error: None,
                }),
                json!({"type": "snapshot", "payload": {
                    "connection": "connecting",
                    "lastStatus": null,
                    "events": [],
                    "isBusy": false,
                    "error": null
                }}),
            ),
        ];
        for (event, expected) in cases {
            assert_eq!(serde_json::to_value(&event).unwrap(), expected);
        }
    }

    #[test]
    fn partial_parameters_merge_over_defaults() {
        let body: CommandRequestBody = serde_json::from_value(json!({
            "ttl_ms": 5000,
            "parameters": {"speed": 0.3, "target": "bar"}
        }))
        .unwrap();
        let params = body
            .parameters
            .unwrap()
            .merge_over(default_move_parameters());
        assert!(matches!(params.target, Target::Bar));
        assert_eq!(params.speed, 0.3);
        assert!(matches!(params.direction, Direction::Forward));
        assert_eq!(params.stop_on_tag, "tag_07");
    }

    #[test]
    fn manual_action_payloads() {
        for (raw, expected) in [("stop", "0"), ("steps", "1"), ("run", "2")] {
            let action: ManualAction = serde_json::from_value(json!(raw)).unwrap();
            assert_eq!(action.firmware_payload(), expected);
        }
        assert!(serde_json::from_value::<ManualAction>(json!("fly")).is_err());
    }
}
