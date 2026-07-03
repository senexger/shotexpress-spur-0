//! MQTT wire types shared by the control server and the train simulator.
//!
//! Source of truth for the message shapes is `webserver/lib/schemas.ts`
//! (`exec_type` discriminator); the root README's spec is stale where it says
//! `event_type`/`started`.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

pub const TOPIC_COMMAND: &str = "shotexpress/command";
pub const TOPIC_STATUS: &str = "shotexpress/status";
pub const TOPIC_EVENT_EXEC: &str = "shotexpress/event/exec";

pub const DEFAULT_TTL_MS: u64 = 60_000;

pub fn default_move_parameters() -> MoveCommandParameters {
    MoveCommandParameters {
        target: Target::Raucherecke,
        speed: 0.6,
        direction: Direction::Forward,
        expected_tags: ["tag_02", "tag_03", "tag_04", "tag_05", "tag_06"]
            .map(String::from)
            .to_vec(),
        stop_on_tag: "tag_07".to_string(),
        max_run_ms_without_tag: 7_000,
    }
}

// ---- envelope ----

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub msg_id: Uuid,
    pub cmd_id: CmdId,
    pub seq: u64,
    pub ts_ms: u64,
}

/// The TS schema allows `cmd_id` to be a uuid or the empty string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmdId {
    None,
    Id(Uuid),
}

impl Serialize for CmdId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            CmdId::None => serializer.serialize_str(""),
            CmdId::Id(id) => id.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for CmdId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        if raw.is_empty() {
            return Ok(CmdId::None);
        }
        Uuid::parse_str(&raw).map(CmdId::Id).map_err(|_| {
            serde::de::Error::custom(format!("cmd_id is neither a uuid nor empty: {raw}"))
        })
    }
}

// ---- exec events (shotexpress/event/exec) ----

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecEvent {
    #[serde(flatten)]
    pub envelope: Envelope,
    #[serde(flatten)]
    pub kind: ExecKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "exec_type", rename_all = "snake_case")]
pub enum ExecKind {
    Accepted,
    Progress {
        direction: Direction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        progress: Option<ProgressDetails>,
    },
    Completed,
    Failed {
        error: ExecError,
    },
    Cancelled,
    Expired,
}

impl ExecKind {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            ExecKind::Completed | ExecKind::Failed { .. } | ExecKind::Cancelled | ExecKind::Expired
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance_m: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecError {
    pub code: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Forward,
    Reverse,
}

impl Direction {
    pub fn reversed(self) -> Direction {
        match self {
            Direction::Forward => Direction::Reverse,
            Direction::Reverse => Direction::Forward,
        }
    }
}

// ---- status (shotexpress/status) ----

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusMessage {
    #[serde(flatten)]
    pub envelope: Envelope,
    pub state: TrainState,
    pub battery_pct: u8,
    pub status_uptime_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TrainState {
    Idle,
    Running,
    Approach,
    Dwell,
    SafeStop,
    Error,
}

impl TrainState {
    pub fn as_str(&self) -> &'static str {
        match self {
            TrainState::Idle => "IDLE",
            TrainState::Running => "RUNNING",
            TrainState::Approach => "APPROACH",
            TrainState::Dwell => "DWELL",
            TrainState::SafeStop => "SAFE_STOP",
            TrainState::Error => "ERROR",
        }
    }
}

// ---- commands (shotexpress/command) ----

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandMessage {
    #[serde(flatten)]
    pub envelope: Envelope,
    #[serde(flatten)]
    pub kind: CommandKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd_type", rename_all = "snake_case")]
pub enum CommandKind {
    Stop,
    WaitForLoad {
        wait_time_ms: u64,
        ttl_ms: u64,
    },
    MoveTo {
        params: MoveCommandParameters,
        ttl_ms: u64,
    },
    Continue {
        params: MoveCommandParameters,
        ttl_ms: u64,
    },
}

impl CommandKind {
    pub fn name(&self) -> &'static str {
        match self {
            CommandKind::Stop => "stop",
            CommandKind::WaitForLoad { .. } => "wait_for_load",
            CommandKind::MoveTo { .. } => "move_to",
            CommandKind::Continue { .. } => "continue",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveCommandParameters {
    pub target: Target,
    pub speed: f64,
    pub direction: Direction,
    pub expected_tags: Vec<String>,
    pub stop_on_tag: String,
    pub max_run_ms_without_tag: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    Bar,
    Schachbrett,
    ForgotName,
    Raucherecke,
}

impl Target {
    pub fn as_str(&self) -> &'static str {
        match self {
            Target::Bar => "bar",
            Target::Schachbrett => "schachbrett",
            Target::ForgotName => "forgot_name",
            Target::Raucherecke => "raucherecke",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn envelope() -> Envelope {
        Envelope {
            msg_id: Uuid::parse_str("11111111-2222-4333-8444-555555555555").unwrap(),
            cmd_id: CmdId::Id(Uuid::parse_str("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee").unwrap()),
            seq: 3,
            ts_ms: 1_751_500_000_000,
        }
    }

    #[test]
    fn exec_event_variants_round_trip() {
        let cases = [
            json!({
                "msg_id": "11111111-2222-4333-8444-555555555555",
                "cmd_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                "seq": 1, "ts_ms": 1751500000000u64,
                "exec_type": "accepted"
            }),
            json!({
                "msg_id": "11111111-2222-4333-8444-555555555555",
                "cmd_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                "seq": 2, "ts_ms": 1751500000000u64,
                "exec_type": "progress",
                "direction": "forward"
            }),
            json!({
                "msg_id": "11111111-2222-4333-8444-555555555555",
                "cmd_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                "seq": 3, "ts_ms": 1751500000000u64,
                "exec_type": "progress",
                "direction": "forward",
                "progress": {"last_tag": "tag_03", "distance_m": 2.1}
            }),
            json!({
                "msg_id": "11111111-2222-4333-8444-555555555555",
                "cmd_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                "seq": 4, "ts_ms": 1751500000000u64,
                "exec_type": "completed"
            }),
            json!({
                "msg_id": "11111111-2222-4333-8444-555555555555",
                "cmd_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                "seq": 5, "ts_ms": 1751500000000u64,
                "exec_type": "failed",
                "error": {"code": "UNROUTABLE", "reason": "unknown target"}
            }),
            json!({
                "msg_id": "11111111-2222-4333-8444-555555555555",
                "cmd_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                "seq": 6, "ts_ms": 1751500000000u64,
                "exec_type": "cancelled"
            }),
            json!({
                "msg_id": "11111111-2222-4333-8444-555555555555",
                "cmd_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                "seq": 7, "ts_ms": 1751500000000u64,
                "exec_type": "expired"
            }),
        ];

        for case in cases {
            let event: ExecEvent = serde_json::from_value(case.clone())
                .unwrap_or_else(|e| panic!("failed to parse {case}: {e}"));
            let back = serde_json::to_value(&event).unwrap();
            assert_eq!(back, case);
        }
    }

    #[test]
    fn exec_event_terminal_detection() {
        assert!(!ExecKind::Accepted.is_terminal());
        assert!(
            !ExecKind::Progress {
                direction: Direction::Forward,
                progress: None
            }
            .is_terminal()
        );
        assert!(ExecKind::Completed.is_terminal());
        assert!(ExecKind::Cancelled.is_terminal());
        assert!(ExecKind::Expired.is_terminal());
        assert!(
            ExecKind::Failed {
                error: ExecError {
                    code: "X".into(),
                    reason: "y".into()
                }
            }
            .is_terminal()
        );
    }

    #[test]
    fn cmd_id_empty_string_round_trip() {
        let raw = json!({
            "msg_id": "11111111-2222-4333-8444-555555555555",
            "cmd_id": "",
            "seq": 0, "ts_ms": 0,
            "exec_type": "accepted"
        });
        let event: ExecEvent = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(event.envelope.cmd_id, CmdId::None);
        assert_eq!(serde_json::to_value(&event).unwrap(), raw);

        let bad = json!({
            "msg_id": "11111111-2222-4333-8444-555555555555",
            "cmd_id": "not-a-uuid",
            "seq": 0, "ts_ms": 0,
            "exec_type": "accepted"
        });
        assert!(serde_json::from_value::<ExecEvent>(bad).is_err());
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let raw = json!({
            "msg_id": "11111111-2222-4333-8444-555555555555",
            "cmd_id": "",
            "seq": 0, "ts_ms": 0,
            "exec_type": "accepted",
            "some_future_field": {"a": 1}
        });
        assert!(serde_json::from_value::<ExecEvent>(raw).is_ok());

        let raw = json!({
            "msg_id": "11111111-2222-4333-8444-555555555555",
            "cmd_id": "",
            "seq": 0, "ts_ms": 0,
            "state": "IDLE", "battery_pct": 86, "status_uptime_ms": 1234,
            "extra": true
        });
        assert!(serde_json::from_value::<StatusMessage>(raw).is_ok());

        let raw = json!({
            "msg_id": "11111111-2222-4333-8444-555555555555",
            "cmd_id": "",
            "seq": 0, "ts_ms": 0,
            "cmd_type": "stop",
            "extra": true
        });
        assert!(serde_json::from_value::<CommandMessage>(raw).is_ok());
    }

    #[test]
    fn status_message_round_trip() {
        let raw = json!({
            "msg_id": "11111111-2222-4333-8444-555555555555",
            "cmd_id": "",
            "seq": 12, "ts_ms": 1751500000000u64,
            "state": "SAFE_STOP", "battery_pct": 86, "status_uptime_ms": 4200
        });
        let status: StatusMessage = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(status.state, TrainState::SafeStop);
        assert_eq!(serde_json::to_value(&status).unwrap(), raw);
        // floats and out-of-range are rejected like zod's int()/max(100)
        assert!(
            serde_json::from_value::<StatusMessage>(json!({
                "msg_id": "11111111-2222-4333-8444-555555555555",
                "cmd_id": "", "seq": 0, "ts_ms": 0,
                "state": "IDLE", "battery_pct": 300, "status_uptime_ms": 0
            }))
            .is_err()
        );
    }

    #[test]
    fn command_message_matches_ts_wire_format() {
        // Golden shape mirroring webserver/lib/messages.ts createMoveCommand output
        let command = CommandMessage {
            envelope: envelope(),
            kind: CommandKind::MoveTo {
                params: default_move_parameters(),
                ttl_ms: DEFAULT_TTL_MS,
            },
        };
        let expected = json!({
            "msg_id": "11111111-2222-4333-8444-555555555555",
            "cmd_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            "seq": 3,
            "ts_ms": 1751500000000u64,
            "cmd_type": "move_to",
            "params": {
                "target": "raucherecke",
                "speed": 0.6,
                "direction": "forward",
                "expected_tags": ["tag_02", "tag_03", "tag_04", "tag_05", "tag_06"],
                "stop_on_tag": "tag_07",
                "max_run_ms_without_tag": 7000
            },
            "ttl_ms": 60000
        });
        assert_eq!(serde_json::to_value(&command).unwrap(), expected);

        // and deserializes back to the same wire shape (simulator's parse path)
        let parsed: CommandMessage = serde_json::from_value(expected.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), expected);
        assert!(matches!(
            parsed.kind,
            CommandKind::MoveTo { ttl_ms: 60000, .. }
        ));
    }

    #[test]
    fn command_variants_round_trip() {
        let cases = [
            json!({
                "msg_id": "11111111-2222-4333-8444-555555555555",
                "cmd_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                "seq": 1, "ts_ms": 1751500000000u64,
                "cmd_type": "stop"
            }),
            json!({
                "msg_id": "11111111-2222-4333-8444-555555555555",
                "cmd_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                "seq": 2, "ts_ms": 1751500000000u64,
                "cmd_type": "wait_for_load",
                "wait_time_ms": 5000, "ttl_ms": 15000
            }),
            json!({
                "msg_id": "11111111-2222-4333-8444-555555555555",
                "cmd_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                "seq": 3, "ts_ms": 1751500000000u64,
                "cmd_type": "continue",
                "params": {
                    "target": "bar",
                    "speed": 0.4,
                    "direction": "reverse",
                    "expected_tags": ["tag_02"],
                    "stop_on_tag": "tag_01",
                    "max_run_ms_without_tag": 7000
                },
                "ttl_ms": 15000
            }),
        ];
        for case in cases {
            let command: CommandMessage = serde_json::from_value(case.clone())
                .unwrap_or_else(|e| panic!("failed to parse {case}: {e}"));
            assert_eq!(serde_json::to_value(&command).unwrap(), case);
        }
    }

    #[test]
    fn raw_and_garbage_payloads_fail_to_parse_as_commands() {
        // the simulator falls back to the raw firmware protocol for these
        for raw in ["0", "1", "2", "Alive", "", "{\"cmd_type\":\"fly\"}"] {
            assert!(
                serde_json::from_slice::<CommandMessage>(raw.as_bytes()).is_err(),
                "{raw:?} should not parse as CommandMessage"
            );
        }
    }
}
