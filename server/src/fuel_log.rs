//! Fuel station measurement log, for judging later how reliably the
//! ultrasonic sensor sees a glass fill.
//!
//! One JSON Lines file per server start: `$FUEL_LOG_DIR/fuel-<ts_ms>.jsonl`
//! (default dir `logs/`). Every line has `ts_ms` (server clock) and `kind`:
//!
//! - `session_start` `{version}`
//! - `mqtt`          `{connected}` — gaps in the data show up here
//! - `dispense_sent` `{pour, cmd_id, volume_ml}` — `pour` counts up from 1
//! - `stop_sent`     `{pour, cmd_id}` — `pour` = the running pour or null
//! - `sensor`        `{mm, pour, pouring, t_pour_ms}` — mm = -1 is no echo;
//!   `pour` stays set after the pour ended (`pouring: false`) so the settled
//!   reading after `completed` belongs to that pour too
//! - `fuel_event`    `{pour, cmd_id, exec_type, error_code, error_reason, station_ts_ms, seq}`
//! - `fuel_status`   `{state, tank_pct, cmd_id}`
//! - `tag`           `{tag, pour, pouring}` — raw uid from the train's tag reader
//! - `parse_error`   `{topic, payload}`
//!
//! Example: `jq -c 'select(.kind=="sensor" and .pour==3)' logs/fuel-*.jsonl`

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::types::{CmdId, FuelEvent, FuelEventKind, FuelStatusMessage, now_ms};

const FORMAT_VERSION: u32 = 1;

struct ActivePour {
    pour: u64,
    cmd_id: Uuid,
}

pub struct FuelLog {
    /// None when the file could not be opened: everything becomes a no-op.
    writer: Option<BufWriter<File>>,
    /// Number of the latest pour (0 = none yet).
    pour: u64,
    pour_sent_ms: u64,
    active: Option<ActivePour>,
}

impl FuelLog {
    pub fn open(dir: &Path) -> Self {
        let path = dir.join(format!("fuel-{}.jsonl", now_ms()));
        let writer = fs::create_dir_all(dir)
            .and_then(|()| File::options().create(true).append(true).open(&path));
        let writer = match writer {
            Ok(file) => {
                tracing::info!(path = %path.display(), "fuel log opened");
                Some(BufWriter::new(file))
            }
            Err(err) => {
                tracing::warn!(%err, path = %path.display(), "cannot open fuel log, logging disabled");
                None
            }
        };
        let mut log = FuelLog {
            writer,
            pour: 0,
            pour_sent_ms: 0,
            active: None,
        };
        log.write("session_start", json!({ "version": FORMAT_VERSION }));
        log
    }

    pub fn mqtt(&mut self, connected: bool) {
        self.write("mqtt", json!({ "connected": connected }));
    }

    pub fn dispense_sent(&mut self, cmd_id: Uuid, volume_ml: u32) {
        self.pour += 1;
        self.pour_sent_ms = now_ms();
        self.active = Some(ActivePour {
            pour: self.pour,
            cmd_id,
        });
        self.write(
            "dispense_sent",
            json!({ "pour": self.pour, "cmd_id": cmd_id, "volume_ml": volume_ml }),
        );
    }

    pub fn stop_sent(&mut self, cmd_id: Uuid) {
        let pour = self.active.as_ref().map(|a| a.pour);
        self.write("stop_sent", json!({ "pour": pour, "cmd_id": cmd_id }));
    }

    pub fn sensor(&mut self, mm: i32) {
        let (pour, t_pour_ms) = self.latest_pour();
        self.write(
            "sensor",
            json!({
                "mm": mm,
                "pour": pour,
                "pouring": self.active.is_some(),
                "t_pour_ms": t_pour_ms,
            }),
        );
    }

    pub fn tag(&mut self, tag: &str) {
        let (pour, _) = self.latest_pour();
        self.write(
            "tag",
            json!({ "tag": tag, "pour": pour, "pouring": self.active.is_some() }),
        );
    }

    pub fn fuel_event(&mut self, event: &FuelEvent) {
        let cmd_id = match event.envelope.cmd_id {
            CmdId::Id(id) => Some(id),
            CmdId::None => None,
        };
        // pours are matched by cmd_id; events for other commands get pour: null
        let pour = match &self.active {
            Some(active) if Some(active.cmd_id) == cmd_id => Some(active.pour),
            _ => None,
        };
        let (exec_type, error_code, error_reason) = match &event.kind {
            FuelEventKind::Accepted => ("accepted", None, None),
            FuelEventKind::Completed => ("completed", None, None),
            FuelEventKind::Failed { error } => {
                ("failed", Some(error.code.as_str()), Some(error.reason.as_str()))
            }
            FuelEventKind::Cancelled => ("cancelled", None, None),
            FuelEventKind::Expired => ("expired", None, None),
        };
        self.write(
            "fuel_event",
            json!({
                "pour": pour,
                "cmd_id": cmd_id,
                "exec_type": exec_type,
                "error_code": error_code,
                "error_reason": error_reason,
                "station_ts_ms": event.envelope.ts_ms,
                "seq": event.envelope.seq,
            }),
        );
        if pour.is_some() && event.kind.is_terminal() {
            self.active = None;
        }
    }

    pub fn fuel_status(&mut self, status: &FuelStatusMessage) {
        self.write(
            "fuel_status",
            json!({
                "state": status.state,
                "tank_pct": status.tank_pct,
                "cmd_id": status.envelope.cmd_id,
            }),
        );
    }

    pub fn parse_error(&mut self, topic: &str, payload: &[u8]) {
        self.write(
            "parse_error",
            json!({ "topic": topic, "payload": String::from_utf8_lossy(payload) }),
        );
    }

    fn latest_pour(&self) -> (Option<u64>, Option<u64>) {
        if self.pour == 0 {
            return (None, None);
        }
        (
            Some(self.pour),
            Some(now_ms().saturating_sub(self.pour_sent_ms)),
        )
    }

    fn write(&mut self, kind: &str, fields: Value) {
        let Some(writer) = &mut self.writer else {
            return;
        };
        let mut line = json!({ "ts_ms": now_ms(), "kind": kind });
        if let (Value::Object(line), Value::Object(fields)) = (&mut line, fields) {
            line.extend(fields);
        }
        // flush per line: the log must survive a killed server
        let result = serde_json::to_writer(&mut *writer, &line)
            .map_err(std::io::Error::from)
            .and_then(|()| writer.write_all(b"\n"))
            .and_then(|()| writer.flush());
        if let Err(err) = result {
            tracing::warn!(%err, "fuel log write failed, logging disabled");
            self.writer = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Envelope;

    fn event(cmd_id: Uuid, kind: FuelEventKind) -> FuelEvent {
        FuelEvent {
            envelope: Envelope {
                msg_id: Uuid::new_v4(),
                cmd_id: CmdId::Id(cmd_id),
                seq: 1,
                ts_ms: 42,
            },
            kind,
        }
    }

    #[test]
    fn tags_lines_with_pour_counter() {
        let dir = std::env::temp_dir().join(format!("fuel-log-test-{}", Uuid::new_v4()));
        let mut log = FuelLog::open(&dir);
        log.sensor(180);
        let cmd = Uuid::new_v4();
        log.dispense_sent(cmd, 20);
        log.fuel_event(&event(cmd, FuelEventKind::Accepted));
        log.sensor(150);
        log.tag("93E0CD1C");
        log.fuel_event(&event(Uuid::new_v4(), FuelEventKind::Completed));
        log.fuel_event(&event(cmd, FuelEventKind::Completed));
        log.sensor(-1);

        let file = fs::read_dir(&dir).unwrap().next().unwrap().unwrap().path();
        let lines: Vec<Value> = fs::read_to_string(&file)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        fs::remove_dir_all(&dir).unwrap();

        let kinds: Vec<&str> = lines.iter().map(|l| l["kind"].as_str().unwrap()).collect();
        assert_eq!(
            kinds,
            [
                "session_start", "sensor", "dispense_sent", "fuel_event", "sensor", "tag",
                "fuel_event", "fuel_event", "sensor"
            ]
        );
        assert_eq!(lines[1]["pour"], Value::Null);
        assert_eq!(lines[1]["pouring"], false);
        assert_eq!(lines[2]["pour"], 1);
        assert_eq!(lines[2]["volume_ml"], 20);
        assert_eq!(lines[3]["exec_type"], "accepted");
        assert_eq!(lines[4]["pour"], 1);
        assert_eq!(lines[4]["pouring"], true);
        assert!(lines[4]["t_pour_ms"].is_u64());
        assert_eq!(lines[5]["tag"], "93E0CD1C");
        assert_eq!(lines[5]["pouring"], true);
        // event for a foreign cmd_id: not part of the pour, pour keeps running
        assert_eq!(lines[6]["pour"], Value::Null);
        assert_eq!(lines[7]["pour"], 1);
        assert_eq!(lines[7]["station_ts_ms"], 42);
        assert_eq!(lines[8]["mm"], -1);
        assert_eq!(lines[8]["pour"], 1);
        assert_eq!(lines[8]["pouring"], false);
    }
}
