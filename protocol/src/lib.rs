//! MQTT wire types shared by the control server, the train simulator and
//! (eventually) the real firmware.
//!
//! Design rule: the train is a dumb executor. It receives *data*, never
//! logic — a `drive` command is an ordered list of legs ("drive at `speed`
//! until you read `tag`"), compiled by the server from its track map. The
//! train holds no map, runs autonomous rides forward-only (passive
//! turnaround switches), and must be able to finish any single command
//! with zero connectivity. `direction` exists only on `steps`, the
//! precise-positioning primitive used under live server control (refill).

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

pub const TOPIC_COMMAND: &str = "shotexpress/command";
pub const TOPIC_STATUS: &str = "shotexpress/status";
pub const TOPIC_EVENT_EXEC: &str = "shotexpress/event/exec";
/// Every RFID read, as a plain-text payload holding just the tag id — no
/// JSON, no envelope, published even outside commands (manual driving).
/// Setup aid for mapping physical tag uids to track.toml names; the server
/// must not use it for localization (backward moves produce reversed pairs).
pub const TOPIC_EVENT_TAG: &str = "shotexpress/event/tag";
/// Max stepper speed for the raw firmware commands, as a plain-text integer
/// in steps/s — no JSON, no envelope. Published retained by the server so
/// the train picks the last value up again after a reboot. Test setup.
pub const TOPIC_SPEED: &str = "shotexpress/speed";
/// Relative move for the raw firmware, as a plain-text signed integer in
/// steps (negative = backward), at the current speed. Not retained: it is
/// an action, not a setting. Test setup.
pub const TOPIC_STEPS: &str = "shotexpress/steps";
/// Start acceleration for the raw firmware, plain-text integer in steps/s².
/// Retained like the speed. Stops stay instant. Test setup.
pub const TOPIC_ACCEL: &str = "shotexpress/accel";

pub const DEFAULT_TTL_MS: u64 = 60_000;

/// Milliseconds since the unix epoch — the clock every `ts_ms` field uses.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_millis() as u64
}

// ---- envelope ----

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub msg_id: Uuid,
    pub cmd_id: CmdId,
    pub seq: u64,
    pub ts_ms: u64,
}

/// `cmd_id` on the wire is a uuid or the empty string (status heartbeats
/// outside a command carry no cmd_id).
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
        last_tag: String,
        distance_m: f64,
        /// Tags the reader missed since the previous progress event; the
        /// train resynced forward past them (harmless on a one-way ring).
        #[serde(default, skip_serializing_if = "is_zero")]
        skipped: u32,
    },
    Completed,
    Failed {
        error: ExecError,
    },
    Cancelled,
    Expired,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl ExecKind {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            ExecKind::Completed | ExecKind::Failed { .. } | ExecKind::Cancelled | ExecKind::Expired
        )
    }
}

/// Well-known `ExecError::code` values emitted by the train:
/// `RFID_TIMEOUT` (no tag within `max_run_ms_without_tag`, safe-stopped),
/// `OVERSHOT` (missed the stop tag, safe-stopped one tag past the target),
/// `UNEXPECTED_TAG` (tag not in the remaining legs, safe-stopped).
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

/// One leg of a drive: hold `speed` until `tag` is read. The speed applies
/// on the track *leading to* the tag, so slowing before a curve or station
/// is a matter of tag placement, decided by the server's map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Leg {
    pub tag: String,
    pub speed: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd_type", rename_all = "snake_case")]
pub enum CommandKind {
    Stop,
    /// Autonomous ride, forward-only. The last leg's tag is the stop tag;
    /// empty `legs` means "come to a stop" and completes immediately.
    /// Offline-completable: everything needed is in the payload.
    Drive {
        legs: Vec<Leg>,
        /// Speed ramp rate in normalized speed units per second (gentle
        /// ramps keep the shots in their glasses).
        accel: f64,
        max_run_ms_without_tag: u64,
        ttl_ms: u64,
    },
    /// Precise positioning by step count. The only command that may go
    /// backward — sent exclusively under live server control (refill at
    /// the bar), never as part of an autonomous ride.
    Steps {
        steps: u32,
        direction: Direction,
        speed: f64,
        ttl_ms: u64,
    },
}

impl CommandKind {
    pub fn name(&self) -> &'static str {
        match self {
            CommandKind::Stop => "stop",
            CommandKind::Drive { .. } => "drive",
            CommandKind::Steps { .. } => "steps",
        }
    }
}

// ---- fuel station (shotexpress/fuel/*) ----
//
// The refill dispenser at the bar is a second dumb executor on the same
// broker, following the train's pattern: it receives a self-contained
// `dispense` command, pours, and reports the outcome as lifecycle events.
// The server choreographs refills (position the train with `steps`, then
// dispense one glass at a time); the wire shape is fixed here even though
// the server-side choreography lands later.

pub const TOPIC_FUEL_COMMAND: &str = "shotexpress/fuel/command";
pub const TOPIC_FUEL_EVENT: &str = "shotexpress/fuel/event";
pub const TOPIC_FUEL_STATUS: &str = "shotexpress/fuel/status";
/// Raw ultrasonic distance in mm as a plain-text integer (-1 = no echo),
/// about 4 Hz, no envelope. Diagnostics only (watch a glass fill,
/// calibrate thresholds), like the train's raw tag topic.
pub const TOPIC_FUEL_SENSOR: &str = "shotexpress/fuel/sensor";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FuelCommandMessage {
    #[serde(flatten)]
    pub envelope: Envelope,
    #[serde(flatten)]
    pub kind: FuelCommandKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd_type", rename_all = "snake_case")]
pub enum FuelCommandKind {
    /// Pour `volume_ml` into whatever sits under the spout. Offline-
    /// completable: once accepted, the pour finishes without connectivity.
    Dispense { volume_ml: u32, ttl_ms: u64 },
    /// Close the valve immediately (`cancelled` if a pour was active).
    Stop,
}

/// Dispenser lifecycle events, mirroring the train's exec events (same
/// envelope, same `exec_type` discriminator, minus `progress`).
/// Well-known `ExecError::code` values: `TANK_EMPTY`, `POUR_TIMEOUT`,
/// `NO_GLASS` (nothing under the spout, or the glass left mid-pour).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FuelEvent {
    #[serde(flatten)]
    pub envelope: Envelope,
    #[serde(flatten)]
    pub kind: FuelEventKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "exec_type", rename_all = "snake_case")]
pub enum FuelEventKind {
    Accepted,
    Completed,
    Failed { error: ExecError },
    Cancelled,
    Expired,
}

impl FuelEventKind {
    pub fn is_terminal(&self) -> bool {
        !matches!(self, FuelEventKind::Accepted)
    }
}

/// Fuel status heartbeat (1 Hz), like the train's.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FuelStatusMessage {
    #[serde(flatten)]
    pub envelope: Envelope,
    pub state: FuelState,
    /// Remaining reservoir, 0-100 (a rough estimate is fine).
    pub tank_pct: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FuelState {
    Idle,
    Dispensing,
    Error,
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
                "last_tag": "track12", "distance_m": 2.4
            }),
            json!({
                "msg_id": "11111111-2222-4333-8444-555555555555",
                "cmd_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                "seq": 3, "ts_ms": 1751500000000u64,
                "exec_type": "progress",
                "last_tag": "station2", "distance_m": 3.6, "skipped": 1
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
                "error": {"code": "OVERSHOT", "reason": "missed stop tag station3"}
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
                last_tag: "track12".into(),
                distance_m: 0.0,
                skipped: 0
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
    fn drive_command_golden_wire_shape() {
        let command = CommandMessage {
            envelope: envelope(),
            kind: CommandKind::Drive {
                legs: vec![
                    Leg {
                        tag: "track12".into(),
                        speed: 0.8,
                    },
                    Leg {
                        tag: "curveA2".into(),
                        speed: 0.3,
                    },
                    Leg {
                        tag: "station3".into(),
                        speed: 0.25,
                    },
                ],
                accel: 0.5,
                max_run_ms_without_tag: 7_000,
                ttl_ms: DEFAULT_TTL_MS,
            },
        };
        let expected = json!({
            "msg_id": "11111111-2222-4333-8444-555555555555",
            "cmd_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            "seq": 3,
            "ts_ms": 1751500000000u64,
            "cmd_type": "drive",
            "legs": [
                {"tag": "track12", "speed": 0.8},
                {"tag": "curveA2", "speed": 0.3},
                {"tag": "station3", "speed": 0.25}
            ],
            "accel": 0.5,
            "max_run_ms_without_tag": 7000,
            "ttl_ms": 60000
        });
        assert_eq!(serde_json::to_value(&command).unwrap(), expected);

        let parsed: CommandMessage = serde_json::from_value(expected.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), expected);
        assert!(matches!(
            parsed.kind,
            CommandKind::Drive { ttl_ms: 60000, .. }
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
                "cmd_type": "drive",
                "legs": [],
                "accel": 0.5,
                "max_run_ms_without_tag": 7000,
                "ttl_ms": 15000
            }),
            json!({
                "msg_id": "11111111-2222-4333-8444-555555555555",
                "cmd_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                "seq": 3, "ts_ms": 1751500000000u64,
                "cmd_type": "steps",
                "steps": 1000, "direction": "reverse", "speed": 0.2,
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
    fn fuel_command_golden_wire_shape() {
        let command = FuelCommandMessage {
            envelope: envelope(),
            kind: FuelCommandKind::Dispense {
                volume_ml: 40,
                ttl_ms: 15_000,
            },
        };
        let expected = json!({
            "msg_id": "11111111-2222-4333-8444-555555555555",
            "cmd_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            "seq": 3,
            "ts_ms": 1751500000000u64,
            "cmd_type": "dispense",
            "volume_ml": 40,
            "ttl_ms": 15000
        });
        assert_eq!(serde_json::to_value(&command).unwrap(), expected);
        let parsed: FuelCommandMessage = serde_json::from_value(expected.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), expected);

        let stop = json!({
            "msg_id": "11111111-2222-4333-8444-555555555555",
            "cmd_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            "seq": 4, "ts_ms": 1751500000000u64,
            "cmd_type": "stop"
        });
        let parsed: FuelCommandMessage = serde_json::from_value(stop.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), stop);
    }

    #[test]
    fn fuel_event_variants_round_trip() {
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
                "exec_type": "completed"
            }),
            json!({
                "msg_id": "11111111-2222-4333-8444-555555555555",
                "cmd_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                "seq": 3, "ts_ms": 1751500000000u64,
                "exec_type": "failed",
                "error": {"code": "TANK_EMPTY", "reason": "reservoir below cutoff"}
            }),
            json!({
                "msg_id": "11111111-2222-4333-8444-555555555555",
                "cmd_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                "seq": 4, "ts_ms": 1751500000000u64,
                "exec_type": "cancelled"
            }),
            json!({
                "msg_id": "11111111-2222-4333-8444-555555555555",
                "cmd_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                "seq": 5, "ts_ms": 1751500000000u64,
                "exec_type": "expired"
            }),
        ];
        for case in cases {
            let event: FuelEvent = serde_json::from_value(case.clone())
                .unwrap_or_else(|e| panic!("failed to parse {case}: {e}"));
            let back = serde_json::to_value(&event).unwrap();
            assert_eq!(back, case);
            assert_eq!(
                event.kind.is_terminal(),
                !matches!(event.kind, FuelEventKind::Accepted)
            );
        }
    }

    #[test]
    fn fuel_status_round_trip() {
        let raw = json!({
            "msg_id": "11111111-2222-4333-8444-555555555555",
            "cmd_id": "",
            "seq": 7, "ts_ms": 1751500000000u64,
            "state": "DISPENSING", "tank_pct": 63
        });
        let status: FuelStatusMessage = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(status.state, FuelState::Dispensing);
        assert_eq!(serde_json::to_value(&status).unwrap(), raw);
    }

    #[test]
    fn raw_and_garbage_payloads_fail_to_parse_as_commands() {
        // the train falls back to the raw firmware protocol for these
        for raw in ["0", "1", "2", "3", "Alive", "", "{\"cmd_type\":\"fly\"}"] {
            assert!(
                serde_json::from_slice::<CommandMessage>(raw.as_bytes()).is_err(),
                "{raw:?} should not parse as CommandMessage"
            );
        }
    }
}
