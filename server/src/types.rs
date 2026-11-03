pub use shotexpress_protocol::*;

use serde::{Deserialize, Serialize};

// ---- server-side view types ----

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
#[serde(rename_all = "camelCase")]
pub struct ControllerSnapshot {
    pub connection: ConnectionState,
    pub last_status: Option<StatusMessage>,
    pub events: Vec<ExecEvent>,
    pub is_busy: bool,
    pub error: Option<String>,
    /// Last ring position the server could pin from progress tags, if any.
    pub last_known_tag: Option<String>,
    /// Last raw RFID read (shotexpress/event/tag), commands or not. Setup
    /// aid — shows the uid of whatever tag the reader just passed.
    pub last_tag_read: Option<TagRead>,
    /// Last non-JSON payload on the status topic. The legacy firmware
    /// heartbeats plain "Alive" — enough to show the train as online even
    /// though it carries no state/battery.
    pub last_raw_status: Option<RawStatus>,
    /// False until the train's first message, and again once the heartbeat
    /// goes quiet for longer than the offline timeout.
    pub train_online: bool,
    /// Last speed on shotexpress/speed (retained, so it survives restarts).
    pub speed_hz: Option<u32>,
    /// Last acceleration on shotexpress/accel (retained, steps/s²).
    pub accel: Option<u32>,
    /// Last lifecycle event from the fuel station (any command).
    pub fuel_last_event: Option<FuelEvent>,
    /// Last fuel station heartbeat.
    pub fuel_status: Option<FuelStatusMessage>,
    /// Last raw ultrasonic distance in mm (-1 = no echo).
    pub fuel_distance_mm: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TagRead {
    pub tag: String,
    /// Server receive time; the raw tag topic carries no envelope.
    pub ts_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RawStatus {
    pub raw: String,
    /// Server receive time; the raw payload carries no envelope.
    pub ts_ms: u64,
}

// ---- SSE / broadcast events ----

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "payload", rename_all = "lowercase")]
pub enum TrainServiceEvent {
    Connection(ConnectionState),
    Status(StatusMessage),
    Events(Vec<ExecEvent>),
    Busy(bool),
    Error(Option<String>),
    Snapshot(ControllerSnapshot),
    #[serde(rename = "tag")]
    TagRead(TagRead),
    RawStatus(RawStatus),
    Liveness(bool),
    Speed(u32),
    Accel(u32),
    FuelEvent(FuelEvent),
    FuelStatus(FuelStatusMessage),
    FuelDistance(i32),
}

// ---- HTTP request bodies ----

/// POST /api/train/command — a ride request. The train holds the tag table;
/// clients (and the drive command) only name the station.
#[derive(Debug, Deserialize)]
pub struct RideRequestBody {
    pub target: String,
    #[serde(default)]
    pub speed: Option<f64>,
    #[serde(default)]
    pub ttl_ms: Option<u64>,
}

/// POST /api/train/steps — a positioning nudge (refill choreography).
#[derive(Debug, Deserialize)]
pub struct StepsRequestBody {
    #[serde(default)]
    pub steps: Option<u32>,
    pub direction: Direction,
    #[serde(default)]
    pub speed: Option<f64>,
    #[serde(default)]
    pub ttl_ms: Option<u64>,
}

/// Raw firmware integers on shotexpress/command, matching Train.ino:
/// 0 = stopMove(), 1 = move(1000 steps), 2 = runBackward() (continuous),
/// 3 = runForward() (continuous). Debug/maintenance only — no cmd_id, no
/// exec events; the acked JSON commands are the real interface.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ManualAction {
    Stop,
    Steps,
    Forward,
    Backward,
}

impl ManualAction {
    pub fn firmware_payload(self) -> &'static str {
        match self {
            ManualAction::Stop => "0",
            ManualAction::Steps => "1",
            ManualAction::Backward => "2",
            ManualAction::Forward => "3",
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
                    last_known_tag: None,
                    last_tag_read: None,
                    last_raw_status: None,
                    train_online: false,
                    speed_hz: None,
                    accel: None,
                    fuel_last_event: None,
                    fuel_status: None,
                    fuel_distance_mm: None,
                }),
                json!({"type": "snapshot", "payload": {
                    "connection": "connecting",
                    "lastStatus": null,
                    "events": [],
                    "isBusy": false,
                    "error": null,
                    "lastKnownTag": null,
                    "lastTagRead": null,
                    "lastRawStatus": null,
                    "trainOnline": false,
                    "speedHz": null,
                    "accel": null,
                    "fuelLastEvent": null,
                    "fuelStatus": null,
                    "fuelDistanceMm": null
                }}),
            ),
            (
                TrainServiceEvent::Speed(5000),
                json!({"type": "speed", "payload": 5000}),
            ),
            (
                TrainServiceEvent::Liveness(true),
                json!({"type": "liveness", "payload": true}),
            ),
            (
                TrainServiceEvent::RawStatus(RawStatus {
                    raw: "Alive".into(),
                    ts_ms: 1_751_500_000_000,
                }),
                json!({"type": "rawstatus", "payload": {
                    "raw": "Alive",
                    "ts_ms": 1751500000000u64
                }}),
            ),
            (
                TrainServiceEvent::TagRead(TagRead {
                    tag: "04A1B2C3".into(),
                    ts_ms: 1_751_500_000_000,
                }),
                json!({"type": "tag", "payload": {
                    "tag": "04A1B2C3",
                    "ts_ms": 1751500000000u64
                }}),
            ),
        ];
        for (event, expected) in cases {
            assert_eq!(serde_json::to_value(&event).unwrap(), expected);
        }
    }

    #[test]
    fn ride_request_parses_with_defaults() {
        let body: RideRequestBody =
            serde_json::from_value(json!({"target": "bar", "speed": 0.3})).unwrap();
        assert_eq!(body.target, "bar");
        assert_eq!(body.speed, Some(0.3));
        assert_eq!(body.ttl_ms, None);
        assert!(serde_json::from_value::<RideRequestBody>(json!({})).is_err());
    }

    #[test]
    fn manual_action_payloads() {
        for (raw, expected) in [
            ("stop", "0"),
            ("steps", "1"),
            ("backward", "2"),
            ("forward", "3"),
        ] {
            let action: ManualAction = serde_json::from_value(json!(raw)).unwrap();
            assert_eq!(action.firmware_payload(), expected);
        }
        assert!(serde_json::from_value::<ManualAction>(json!("fly")).is_err());
    }
}
