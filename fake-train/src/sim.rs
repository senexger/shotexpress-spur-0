//! Headless simulation core: physics, train state machine and MQTT command
//! semantics. No tokio, no network — fully driveable from unit tests with a
//! synthetic clock. Behaves like the target Train.ino firmware contract:
//! a dumb executor for `drive` rides against the cached tag table, and
//! `steps` nudges.
//!
//! The train only ever runs forward on autonomous rides: the passive
//! switches route it around the turnaround loops, so "going back" means
//! continuing forward through the next turnaround. Backward motion exists
//! only for `steps` (refill positioning) and the raw manual "2".
//!
//! Each read is looked up on its own: a speed tag sets the speed, the
//! target's station tag stops, everything else is driven past. A missed
//! stop tag just means another lap. Rides planned with a different table
//! version are refused (`TRACK_MISMATCH`); the RFID watchdog safe-stops.

use std::collections::HashSet;

use rand::RngExt;
use shotexpress_protocol::{
    CmdId, CommandKind, CommandMessage, Direction, Envelope, ExecError, ExecEvent, ExecKind,
    StatusMessage, TagKind, TrackTable, TrainState,
};
use uuid::Uuid;

use crate::belief::{LocalizeOutcome, Localizer};
use crate::track::{EDGES, NODES, READINGS, RingPos, next_node};

const DWELL_MS: f64 = 2_000.0;
/// Step rate at speed 1.0; 1000 steps at speed 0.4 take 2000 ms.
const STEPS_PER_MS_FULL: f64 = 1.25;
const RAW_STEPS: u32 = 1_000;
const RAW_STEPS_SPEED: f64 = 0.4;
const RAW_CONTINUOUS_SPEED: f64 = 0.6;
const MIN_SPEED: f64 = 0.05;
/// Ramp rate for raw/steps modes that don't carry an accel (units/s).
const DEFAULT_ACCEL: f64 = 0.5;
/// Chance to miss a read while the flaky-reader toggle is on.
const FLAKY_MISS_P: f64 = 0.35;

#[derive(Debug)]
pub enum SimOutput {
    Exec(ExecEvent),
    Status(StatusMessage),
    /// A raw tag read for shotexpress/event/tag: fires on every read, even
    /// outside commands, like the firmware's reader loop.
    Tag(String),
    Log(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MotorMode {
    Stopped,
    /// Executing a ride (always forward).
    Drive,
    /// Fixed step count, forward or backward (JSON steps or raw "1").
    Steps {
        remaining_steps: f64,
        backward: bool,
    },
    RawForward,
    RawBackward,
}

/// The command currently holding the train (cancel-and-replace semantics).
#[derive(Debug)]
pub enum Active {
    Drive {
        cmd_id: CmdId,
        target: String,
        /// The target station's tag: reading it is a full stop.
        target_uid: String,
        max_speed: f64,
        ms_since_last_tag: f64,
        max_run_ms_without_tag: u64,
        distance_m: f64,
    },
    Steps {
        cmd_id: CmdId,
    },
}

impl Active {
    pub fn cmd_id(&self) -> CmdId {
        match self {
            Active::Drive { cmd_id, .. } | Active::Steps { cmd_id } => *cmd_id,
        }
    }
}

/// A `drive` command's fields.
pub struct DriveParams {
    pub target: String,
    pub max_speed: f64,
    pub start_speed: f64,
    pub accel: f64,
    pub max_run_ms_without_tag: u64,
    pub track_version: u32,
    pub ttl_ms: u64,
}

pub struct Simulator {
    pub truth: RingPos,
    pub motor: MotorMode,
    /// Current (ramped) speed; approaches `target_speed` at `accel`/s.
    pub speed: f64,
    pub target_speed: f64,
    pub accel: f64,
    pub state: TrainState,
    pub localizer: Localizer,
    /// Tag table from the server (retained shotexpress/track).
    pub table: Option<TrackTable>,
    /// Last speed tag read, ride or not; None until the first one.
    pub zone_speed: Option<f64>,
    pub loaded: bool,
    pub battery_pct: u8,
    pub active: Option<Active>,
    /// Interactive flaky-reader toggle: reads are randomly missed.
    pub flaky: bool,
    /// Deterministically miss the next n reads (test hook).
    pub miss_next_tags: u32,
    seq: u64,
    state_since_ms: u64,
    dwell_remaining_ms: f64,
    seen_msg_ids: HashSet<Uuid>,
}

impl Simulator {
    pub fn new(rng: &mut impl RngExt, now_ms: u64) -> Self {
        Simulator {
            truth: RingPos {
                edge: rng.random_range(0..NODES),
                frac: rng.random_range(0.0..1.0),
            },
            motor: MotorMode::Stopped,
            speed: 0.0,
            target_speed: 0.0,
            accel: DEFAULT_ACCEL,
            state: TrainState::Idle,
            localizer: Localizer::new(),
            table: None,
            zone_speed: None,
            loaded: true,
            battery_pct: 86,
            active: None,
            flaky: false,
            miss_next_tags: 0,
            seq: 0,
            state_since_ms: now_ms,
            dwell_remaining_ms: 0.0,
            seen_msg_ids: HashSet::new(),
        }
    }

    // ---- incoming commands ----

    /// Retained tag table from the server.
    pub fn handle_track(&mut self, payload: &[u8]) -> Vec<SimOutput> {
        match serde_json::from_slice::<TrackTable>(payload) {
            Ok(table) => {
                let log = format!("tag table v{}: {} tags", table.version, table.tags.len());
                self.table = Some(table);
                vec![SimOutput::Log(log)]
            }
            Err(err) => vec![SimOutput::Log(format!("unparseable tag table: {err}"))],
        }
    }

    pub fn handle_payload(&mut self, payload: &[u8], now_ms: u64) -> Vec<SimOutput> {
        let text = String::from_utf8_lossy(payload);
        match text.trim() {
            // raw integer protocol, mirroring Train.ino (debug only)
            "0" => return self.handle_raw_stop(now_ms),
            "1" => return self.handle_raw_steps(now_ms),
            "2" => return self.handle_raw_backward(now_ms),
            "3" => return self.handle_raw_forward(now_ms),
            _ => {}
        }

        let command: CommandMessage = match serde_json::from_slice(payload) {
            Ok(command) => command,
            Err(err) => {
                return vec![SimOutput::Log(format!(
                    "unparseable command payload {text:?}: {err}"
                ))];
            }
        };

        if !self.seen_msg_ids.insert(command.envelope.msg_id) {
            return vec![SimOutput::Log(format!(
                "ignoring duplicate command {}",
                command.envelope.msg_id
            ))];
        }

        match command.kind {
            CommandKind::Drive {
                target,
                max_speed,
                start_speed,
                accel,
                max_run_ms_without_tag,
                track_version,
                ttl_ms,
            } => self.handle_drive(
                &command.envelope,
                DriveParams {
                    target,
                    max_speed,
                    start_speed,
                    accel,
                    max_run_ms_without_tag,
                    track_version,
                    ttl_ms,
                },
                now_ms,
            ),
            CommandKind::Steps {
                steps,
                direction,
                speed,
                ttl_ms,
            } => self.handle_steps(&command.envelope, steps, direction, speed, ttl_ms, now_ms),
            CommandKind::Stop => {
                let mut out = vec![SimOutput::Log("cmd stop".into())];
                out.extend(self.stop_request(now_ms));
                out
            }
        }
    }

    fn handle_drive(&mut self, envelope: &Envelope, p: DriveParams, now_ms: u64) -> Vec<SimOutput> {
        let mut out = vec![SimOutput::Log(format!(
            "cmd drive to {}, cap {:.2}, ttl={}ms",
            p.target, p.max_speed, p.ttl_ms
        ))];

        if now_ms > envelope.ts_ms.saturating_add(p.ttl_ms) {
            out.push(SimOutput::Exec(self.exec(
                envelope.cmd_id,
                ExecKind::Expired,
                now_ms,
            )));
            return out;
        }

        let refuse = |code: &str, reason: String| ExecKind::Failed {
            error: ExecError {
                code: code.into(),
                reason,
            },
        };
        let target_uid = match &self.table {
            Some(table) if table.version == p.track_version => {
                match table.station_uid(&p.target) {
                    Some(uid) => uid.to_string(),
                    None => {
                        let kind = refuse(
                            "UNKNOWN_STATION",
                            format!("no station {} in the tag table", p.target),
                        );
                        out.push(SimOutput::Exec(self.exec(envelope.cmd_id, kind, now_ms)));
                        return out;
                    }
                }
            }
            other => {
                let have = other
                    .as_ref()
                    .map(|t| format!("v{}", t.version))
                    .unwrap_or_else(|| "none".into());
                let kind = refuse(
                    "TRACK_MISMATCH",
                    format!(
                        "ride planned with tag table v{}, train has {have}",
                        p.track_version
                    ),
                );
                out.push(SimOutput::Exec(self.exec(envelope.cmd_id, kind, now_ms)));
                return out;
            }
        };

        out.extend(self.cancel_active(now_ms));
        out.push(SimOutput::Exec(self.exec(
            envelope.cmd_id,
            ExecKind::Accepted,
            now_ms,
        )));

        self.accel = p.accel.max(0.01);
        let speed = self.zone_speed.unwrap_or(p.start_speed).min(p.max_speed);
        self.target_speed = speed.clamp(MIN_SPEED, 1.0);
        self.motor = MotorMode::Drive;
        self.active = Some(Active::Drive {
            cmd_id: envelope.cmd_id,
            target: p.target,
            target_uid,
            max_speed: p.max_speed,
            ms_since_last_tag: 0.0,
            max_run_ms_without_tag: p.max_run_ms_without_tag,
            distance_m: 0.0,
        });
        self.set_state(TrainState::Running, now_ms, &mut out);
        out
    }

    fn handle_steps(
        &mut self,
        envelope: &Envelope,
        steps: u32,
        direction: Direction,
        speed: f64,
        ttl_ms: u64,
        now_ms: u64,
    ) -> Vec<SimOutput> {
        let backward = direction == Direction::Reverse;
        let mut out = vec![SimOutput::Log(format!(
            "cmd steps {steps} {} speed={speed}",
            if backward { "backward" } else { "forward" },
        ))];

        if now_ms > envelope.ts_ms.saturating_add(ttl_ms) {
            out.push(SimOutput::Exec(self.exec(
                envelope.cmd_id,
                ExecKind::Expired,
                now_ms,
            )));
            return out;
        }

        out.extend(self.cancel_active(now_ms));
        out.push(SimOutput::Exec(self.exec(
            envelope.cmd_id,
            ExecKind::Accepted,
            now_ms,
        )));
        self.accel = DEFAULT_ACCEL;
        self.target_speed = speed.clamp(MIN_SPEED, 1.0);
        self.motor = MotorMode::Steps {
            remaining_steps: steps as f64,
            backward,
        };
        self.active = Some(Active::Steps {
            cmd_id: envelope.cmd_id,
        });
        self.set_state(TrainState::Running, now_ms, &mut out);
        out
    }

    fn handle_raw_stop(&mut self, now_ms: u64) -> Vec<SimOutput> {
        let mut out = vec![SimOutput::Log("raw 0 -> stop".into())];
        out.extend(self.stop_request(now_ms));
        out
    }

    fn handle_raw_steps(&mut self, now_ms: u64) -> Vec<SimOutput> {
        // firmware: move(1000) steps; no exec events (no cmd_id)
        let mut out = vec![SimOutput::Log("raw 1 -> move 1000 steps".into())];
        out.extend(self.cancel_active(now_ms));
        self.accel = DEFAULT_ACCEL;
        self.target_speed = RAW_STEPS_SPEED;
        self.motor = MotorMode::Steps {
            remaining_steps: RAW_STEPS as f64,
            backward: false,
        };
        self.set_state(TrainState::Running, now_ms, &mut out);
        out
    }

    fn handle_raw_backward(&mut self, now_ms: u64) -> Vec<SimOutput> {
        // firmware: runBackward(), continuous; no exec events
        let mut out = vec![SimOutput::Log("raw 2 -> run backward".into())];
        out.extend(self.cancel_active(now_ms));
        self.accel = DEFAULT_ACCEL;
        self.target_speed = RAW_CONTINUOUS_SPEED;
        self.motor = MotorMode::RawBackward;
        self.set_state(TrainState::Running, now_ms, &mut out);
        out
    }

    fn handle_raw_forward(&mut self, now_ms: u64) -> Vec<SimOutput> {
        // firmware: runForward(), continuous; no exec events
        let mut out = vec![SimOutput::Log("raw 3 -> run forward".into())];
        out.extend(self.cancel_active(now_ms));
        self.accel = DEFAULT_ACCEL;
        self.target_speed = RAW_CONTINUOUS_SPEED;
        self.motor = MotorMode::RawForward;
        self.set_state(TrainState::Running, now_ms, &mut out);
        out
    }

    pub fn moving_backward(&self) -> bool {
        matches!(
            self.motor,
            MotorMode::RawBackward | MotorMode::Steps { backward: true, .. }
        )
    }

    pub fn stop_request(&mut self, now_ms: u64) -> Vec<SimOutput> {
        let mut out = self.cancel_active(now_ms);
        self.motor = MotorMode::Stopped;
        self.speed = 0.0;
        self.target_speed = 0.0;
        self.dwell_remaining_ms = 0.0;
        self.set_state(TrainState::Idle, now_ms, &mut out);
        out
    }

    fn cancel_active(&mut self, now_ms: u64) -> Vec<SimOutput> {
        match self.active.take() {
            // the Cancelled event must carry the *active* command's cmd_id,
            // otherwise the server's cmd_id filter drops it and stays busy
            Some(active) => vec![SimOutput::Exec(self.exec(
                active.cmd_id(),
                ExecKind::Cancelled,
                now_ms,
            ))],
            None => vec![],
        }
    }

    pub fn randomize(&mut self, rng: &mut impl RngExt, now_ms: u64) -> Vec<SimOutput> {
        let mut out = self.stop_request(now_ms);
        self.truth = RingPos {
            edge: rng.random_range(0..NODES),
            frac: rng.random_range(0.0..1.0),
        };
        self.localizer.reset();
        out.push(SimOutput::Log(
            "teleported to a random position, belief reset".into(),
        ));
        out
    }

    // ---- time ----

    pub fn tick(&mut self, dt_ms: f64, now_ms: u64) -> Vec<SimOutput> {
        let mut out = Vec::new();

        if self.state == TrainState::Dwell {
            self.dwell_remaining_ms -= dt_ms;
            if self.dwell_remaining_ms <= 0.0 {
                self.set_state(TrainState::Idle, now_ms, &mut out);
            }
            return out;
        }

        // speed ramp toward the target (gentle = no spilled shots)
        if self.motor != MotorMode::Stopped {
            let step = self.accel * dt_ms / 1000.0;
            if self.speed < self.target_speed {
                self.speed = (self.speed + step).min(self.target_speed);
            } else if self.speed > self.target_speed {
                self.speed = (self.speed - step).max(self.target_speed);
            }
        }

        let move_ms = match self.motor {
            MotorMode::Stopped => 0.0,
            MotorMode::Steps {
                remaining_steps, ..
            } => {
                let step_ms = remaining_steps / (STEPS_PER_MS_FULL * self.speed.max(MIN_SPEED));
                dt_ms.min(step_ms)
            }
            _ => dt_ms,
        };

        let crossed = if move_ms > 0.0 {
            self.advance(move_ms, now_ms, &mut out)
        } else {
            false
        };

        if let MotorMode::Steps {
            remaining_steps, ..
        } = &mut self.motor
        {
            *remaining_steps -= move_ms * STEPS_PER_MS_FULL * self.speed.max(MIN_SPEED);
            if *remaining_steps <= 0.0 {
                self.motor = MotorMode::Stopped;
                self.speed = 0.0;
                self.target_speed = 0.0;
                if let Some(Active::Steps { cmd_id }) = self.active.take() {
                    out.push(SimOutput::Exec(self.exec(
                        cmd_id,
                        ExecKind::Completed,
                        now_ms,
                    )));
                }
                self.set_state(TrainState::Idle, now_ms, &mut out);
            }
        }

        // RFID watchdog, drives only: never roll blind for long
        if let Some(Active::Drive {
            ms_since_last_tag,
            max_run_ms_without_tag,
            cmd_id,
            ..
        }) = &mut self.active
        {
            if crossed {
                *ms_since_last_tag = 0.0;
            } else {
                *ms_since_last_tag += dt_ms;
                if *ms_since_last_tag > *max_run_ms_without_tag as f64 {
                    let cmd_id = *cmd_id;
                    let max = *max_run_ms_without_tag;
                    self.safe_stop(now_ms, &mut out);
                    out.push(SimOutput::Exec(self.exec(
                        cmd_id,
                        ExecKind::Failed {
                            error: ExecError {
                                code: "RFID_TIMEOUT".into(),
                                reason: format!("no tag read within {max} ms"),
                            },
                        },
                        now_ms,
                    )));
                }
            }
        }

        out
    }

    fn safe_stop(&mut self, now_ms: u64, out: &mut Vec<SimOutput>) {
        self.active = None;
        self.motor = MotorMode::Stopped;
        self.speed = 0.0;
        self.target_speed = 0.0;
        self.set_state(TrainState::SafeStop, now_ms, out);
    }

    /// Move for `move_ms`, crossing nodes as they come. Returns whether at
    /// least one tag was *read* (missed reads don't feed the watchdog —
    /// the real reader doesn't know what it missed).
    fn advance(&mut self, move_ms: f64, now_ms: u64, out: &mut Vec<SimOutput>) -> bool {
        let backward = self.moving_backward();
        let mut remaining = move_ms;
        let mut read = false;

        while remaining > 0.0 {
            let edge = EDGES[self.truth.edge];
            let edge_ms = edge.nominal_ms as f64 / self.speed.max(MIN_SPEED);
            let time_to_node = if backward {
                self.truth.frac * edge_ms
            } else {
                (1.0 - self.truth.frac) * edge_ms
            };

            if remaining < time_to_node {
                let delta = remaining / edge_ms;
                self.truth.frac += if backward { -delta } else { delta };
                break;
            }

            remaining -= time_to_node;
            let traversed_len = edge.len_m;
            let node = if backward {
                // crossing the node at the start of the current edge
                let node = self.truth.edge;
                self.truth.edge = (node + NODES - 1) % NODES;
                self.truth.frac = 1.0;
                node
            } else {
                let node = next_node(self.truth.edge);
                self.truth.edge = node;
                self.truth.frac = 0.0;
                node
            };

            // the stepper knows its odometer regardless of tag reads
            if let Some(Active::Drive { distance_m, .. }) = &mut self.active {
                *distance_m += traversed_len;
            }

            let (was_read, stop) = self.on_tag(node, backward, now_ms, out);
            read |= was_read;
            if stop {
                break;
            }
        }

        read
    }

    /// Handle crossing the tag at `node`. Returns (tag was read, train
    /// stopped here).
    fn on_tag(
        &mut self,
        node: usize,
        backward: bool,
        now_ms: u64,
        out: &mut Vec<SimOutput>,
    ) -> (bool, bool) {
        let tag = READINGS[node];

        // a missed read: the train never learns this tag happened
        if self.miss_next_tags > 0 {
            self.miss_next_tags -= 1;
            out.push(SimOutput::Log(format!("tag {tag} MISSED (forced)")));
            return (false, false);
        }
        if self.flaky && rand::rng().random_bool(FLAKY_MISS_P) {
            out.push(SimOutput::Log(format!("tag {tag} MISSED (flaky reader)")));
            return (false, false);
        }

        out.push(SimOutput::Tag(tag.to_string()));

        let outcome = if backward {
            self.localizer.on_reading_backward(tag)
        } else {
            self.localizer.on_reading(tag)
        };
        match outcome {
            LocalizeOutcome::Candidates(n) => {
                out.push(SimOutput::Log(format!(
                    "tag {tag} -> {n} candidate positions"
                )));
            }
            LocalizeOutcome::JustLocalized(edge) => {
                out.push(SimOutput::Log(format!(
                    "tag {tag} -> localized on edge {edge}"
                )));
            }
            LocalizeOutcome::Tracking(_) => {
                out.push(SimOutput::Log(format!("tag {tag}")));
            }
            LocalizeOutcome::Mismatch { expected, got } => {
                out.push(SimOutput::Log(format!(
                    "tag {got} contradicts prediction {expected}, relocalizing"
                )));
            }
        }

        // the speed zone is tracked on every forward read, ride or not
        let rule = self.table.as_ref().and_then(|t| t.rule(tag)).cloned();
        if !backward && let Some(TagKind::Speed { speed }) = &rule {
            self.zone_speed = Some(*speed);
        }

        let Some(Active::Drive {
            cmd_id,
            target_uid,
            max_speed,
            ms_since_last_tag,
            distance_m,
            ..
        }) = &mut self.active
        else {
            return (true, false);
        };
        *ms_since_last_tag = 0.0;
        let cmd_id = *cmd_id;
        let max_speed = *max_speed;
        let distance_m = (*distance_m * 10.0).round() / 10.0;

        if tag == target_uid.as_str() {
            // the target's station tag: full stop
            self.active = None;
            self.motor = MotorMode::Stopped;
            self.speed = 0.0;
            self.target_speed = 0.0;
            self.dwell_remaining_ms = DWELL_MS;
            self.set_state(TrainState::Dwell, now_ms, out);
            out.push(SimOutput::Exec(self.exec(
                cmd_id,
                ExecKind::Completed,
                now_ms,
            )));
            return (true, true);
        }

        if let Some(TagKind::Speed { speed }) = rule {
            self.target_speed = speed.min(max_speed).clamp(MIN_SPEED, 1.0);
        }
        // other stations and unlisted tags: drive on
        out.push(SimOutput::Exec(self.exec(
            cmd_id,
            ExecKind::Progress {
                last_tag: tag.to_string(),
                distance_m,
                skipped: 0,
            },
            now_ms,
        )));
        (true, false)
    }

    // ---- outgoing messages ----

    pub fn heartbeat(&mut self, now_ms: u64) -> SimOutput {
        let cmd_id = self
            .active
            .as_ref()
            .map(|a| a.cmd_id())
            .unwrap_or(CmdId::None);
        let envelope = self.next_envelope(cmd_id, now_ms);
        SimOutput::Status(StatusMessage {
            envelope,
            state: self.state,
            battery_pct: self.battery_pct,
            status_uptime_ms: now_ms.saturating_sub(self.state_since_ms),
        })
    }

    fn set_state(&mut self, state: TrainState, now_ms: u64, out: &mut Vec<SimOutput>) {
        if self.state == state {
            return;
        }
        self.state = state;
        self.state_since_ms = now_ms;
        // a status is published immediately on every state change
        out.push(self.heartbeat(now_ms));
    }

    fn exec(&mut self, cmd_id: CmdId, kind: ExecKind, now_ms: u64) -> ExecEvent {
        ExecEvent {
            envelope: self.next_envelope(cmd_id, now_ms),
            kind,
        }
    }

    fn next_envelope(&mut self, cmd_id: CmdId, now_ms: u64) -> Envelope {
        self.seq += 1;
        Envelope {
            msg_id: Uuid::new_v4(),
            cmd_id,
            seq: self.seq,
            ts_ms: now_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::belief::Belief;
    use serde_json::{Value, json};

    const T0: u64 = 1_783_000_000_000;
    const VERSION: u32 = 7;

    /// Mirrors the repo track.toml (stations on the eastbound pass, slow
    /// curves) with the simulator's tag names.
    fn table() -> Vec<u8> {
        json!({
            "version": VERSION,
            "tags": [
                {"uid": "station1", "kind": "station", "station": "bar"},
                {"uid": "station2", "kind": "station", "station": "schachbrett"},
                {"uid": "station3", "kind": "station", "station": "raucherecke"},
                {"uid": "track12", "kind": "speed", "speed": 0.6},
                {"uid": "curveB1", "kind": "speed", "speed": 0.5},
                {"uid": "curveB2", "kind": "speed", "speed": 1.0},
                {"uid": "curveA1", "kind": "speed", "speed": 0.5},
                {"uid": "curveA2", "kind": "speed", "speed": 0.6}
            ]
        })
        .to_string()
        .into_bytes()
    }

    fn sim_at(edge: usize, frac: f64) -> Simulator {
        let mut sim = Simulator::new(&mut rand::rng(), T0);
        sim.truth = RingPos { edge, frac };
        sim.handle_track(&table());
        // tests reason about tag gaps, not ramp physics: start rides at
        // cruise instantly unless a test opts back in
        sim.speed = 0.6;
        sim.accel = 1e9;
        sim
    }

    fn drive(target: &str) -> Value {
        json!({
            "msg_id": Uuid::new_v4().to_string(),
            "cmd_id": Uuid::new_v4().to_string(),
            "seq": 1, "ts_ms": T0,
            "cmd_type": "drive",
            "target": target,
            "max_speed": 1.0,
            "start_speed": 0.6,
            "accel": 1e9,
            "max_run_ms_without_tag": 7000,
            "track_version": VERSION,
            "ttl_ms": 60000
        })
    }

    fn send(sim: &mut Simulator, payload: &Value) -> Vec<SimOutput> {
        sim.handle_payload(payload.to_string().as_bytes(), T0)
    }

    fn steps_payload(steps: u32, direction: &str, speed: f64, ts_ms: u64) -> Vec<u8> {
        json!({
            "msg_id": Uuid::new_v4().to_string(),
            "cmd_id": Uuid::new_v4().to_string(),
            "seq": 1, "ts_ms": ts_ms,
            "cmd_type": "steps",
            "steps": steps, "direction": direction, "speed": speed,
            "ttl_ms": 60000
        })
        .to_string()
        .into_bytes()
    }

    fn exec_kinds(outputs: &[SimOutput]) -> Vec<String> {
        outputs
            .iter()
            .filter_map(|o| match o {
                SimOutput::Exec(e) => Some(match &e.kind {
                    ExecKind::Accepted => "accepted".to_string(),
                    ExecKind::Progress { last_tag, .. } => format!("progress:{last_tag}"),
                    ExecKind::Completed => "completed".to_string(),
                    ExecKind::Failed { error } => format!("failed:{}", error.code),
                    ExecKind::Cancelled => "cancelled".to_string(),
                    ExecKind::Expired => "expired".to_string(),
                }),
                _ => None,
            })
            .collect()
    }

    /// Run the sim in 40 ms steps for up to `ms`, collecting outputs.
    fn run(sim: &mut Simulator, ms: u64) -> Vec<SimOutput> {
        let mut out = Vec::new();
        let mut clock = T0;
        let mut elapsed = 0;
        while elapsed < ms {
            clock += 40;
            elapsed += 40;
            out.extend(sim.tick(40.0, clock));
        }
        out
    }

    /// Run until `tag` is read (panics after 60 s of sim time).
    fn run_until_tag(sim: &mut Simulator, tag: &str) -> Vec<SimOutput> {
        let mut out = Vec::new();
        let mut clock = T0;
        for _ in 0..1500 {
            clock += 40;
            let step = sim.tick(40.0, clock);
            let hit = step.iter().any(|o| matches!(o, SimOutput::Tag(t) if t == tag));
            out.extend(step);
            if hit {
                return out;
            }
        }
        panic!("never read {tag}");
    }

    #[test]
    fn happy_path_drive_to_station() {
        // start just past station1 heading east; stop at station2
        let mut sim = sim_at(0, 0.1);
        let out = send(&mut sim, &drive("schachbrett"));
        assert_eq!(exec_kinds(&out), vec!["accepted"]);
        assert_eq!(sim.state, TrainState::Running);

        let out = run(&mut sim, 30_000);
        assert_eq!(exec_kinds(&out), vec!["progress:track12", "completed"]);
        assert_eq!(sim.state, TrainState::Idle, "back to IDLE after dwell");
        assert!(sim.active.is_none());
        assert_eq!(sim.truth.edge, 2, "stopped on the station2 node");
        let progress_distance = out.iter().find_map(|o| match o {
            SimOutput::Exec(ExecEvent {
                kind: ExecKind::Progress { distance_m, .. },
                ..
            }) => Some(*distance_m),
            _ => None,
        });
        assert_eq!(progress_distance, Some(1.2));
    }

    #[test]
    fn other_stations_are_driven_past() {
        let mut sim = sim_at(0, 0.1);
        send(&mut sim, &drive("raucherecke"));
        let out = run(&mut sim, 30_000);
        assert_eq!(
            exec_kinds(&out),
            vec![
                "progress:track12",
                "progress:station2",
                "progress:track23",
                "completed"
            ]
        );
        assert_eq!(sim.truth.edge, 4);
    }

    #[test]
    fn speed_tags_set_the_speed() {
        let mut sim = sim_at(4, 0.05); // entering loop B
        send(&mut sim, &drive("bar"));
        run_until_tag(&mut sim, "curveB1");
        assert_eq!(sim.target_speed, 0.5, "slow through the turnaround");
        run_until_tag(&mut sim, "curveB2");
        assert_eq!(sim.target_speed, 1.0, "curve done");
        // westbound tags are unlisted: speed stays
        run_until_tag(&mut sim, "west2");
        assert_eq!(sim.target_speed, 1.0);
        let out = run(&mut sim, 40_000);
        assert!(exec_kinds(&out).contains(&"completed".to_string()));
        assert_eq!(sim.truth.edge, 0, "stopped at bar (station1)");
    }

    #[test]
    fn ride_max_speed_caps_tag_speeds() {
        let mut sim = sim_at(5, 0.5);
        let mut cmd = drive("bar");
        cmd["max_speed"] = json!(0.3);
        send(&mut sim, &cmd);
        run_until_tag(&mut sim, "curveB2"); // a 1.0 tag
        assert_eq!(sim.target_speed, 0.3);
    }

    #[test]
    fn start_speed_until_the_zone_is_known() {
        let mut sim = sim_at(0, 0.1);
        send(&mut sim, &drive("schachbrett"));
        assert_eq!(sim.target_speed, 0.6, "start_speed: zone unknown");

        let mut sim = sim_at(0, 0.1);
        sim.zone_speed = Some(0.4);
        send(&mut sim, &drive("schachbrett"));
        assert_eq!(sim.target_speed, 0.4, "remembered zone");
    }

    #[test]
    fn zone_is_tracked_outside_rides() {
        let mut sim = sim_at(4, 0.5);
        sim.handle_payload(b"3", T0); // raw forward, no ride
        run_until_tag(&mut sim, "curveB1");
        assert_eq!(sim.zone_speed, Some(0.5));
    }

    #[test]
    fn missed_stop_tag_means_another_lap() {
        let mut sim = sim_at(0, 0.1);
        send(&mut sim, &drive("schachbrett"));
        run_until_tag(&mut sim, "track12");
        sim.miss_next_tags = 1; // the station2 read fails
        let out = run(&mut sim, 120_000);
        let kinds = exec_kinds(&out);
        assert!(kinds.contains(&"progress:west2".to_string()), "went round");
        assert_eq!(kinds.last().map(String::as_str), Some("completed"));
        assert_eq!(sim.truth.edge, 2);
    }

    #[test]
    fn stale_or_missing_table_is_refused() {
        let mut sim = sim_at(0, 0.1);
        let mut cmd = drive("bar");
        cmd["track_version"] = json!(VERSION + 1);
        let out = send(&mut sim, &cmd);
        assert_eq!(exec_kinds(&out), vec!["failed:TRACK_MISMATCH"]);
        assert!(sim.active.is_none());
        assert_eq!(sim.motor, MotorMode::Stopped);

        let mut sim = sim_at(0, 0.1);
        sim.table = None;
        let out = send(&mut sim, &drive("bar"));
        assert_eq!(exec_kinds(&out), vec!["failed:TRACK_MISMATCH"]);
    }

    #[test]
    fn unknown_station_is_refused() {
        let mut sim = sim_at(0, 0.1);
        let out = send(&mut sim, &drive("mond"));
        assert_eq!(exec_kinds(&out), vec!["failed:UNKNOWN_STATION"]);
        assert!(sim.active.is_none());
    }

    #[test]
    fn steps_command_is_acked_and_moves() {
        let mut sim = sim_at(0, 0.5);
        let out = sim.handle_payload(&steps_payload(1000, "forward", 0.4, T0), T0);
        assert_eq!(exec_kinds(&out), vec!["accepted"]);
        let start = sim.truth;
        let out = run(&mut sim, 6_000);
        assert_eq!(exec_kinds(&out), vec!["completed"]);
        assert_eq!(sim.state, TrainState::Idle);
        assert!(
            sim.truth.frac > start.frac || sim.truth.edge != start.edge,
            "moved forward"
        );

        // backward steps move the other way (refill nudge)
        let start = sim.truth;
        sim.handle_payload(&steps_payload(500, "reverse", 0.4, T0), T0);
        run(&mut sim, 6_000);
        let moved_back = sim.truth.edge != start.edge || sim.truth.frac < start.frac;
        assert!(moved_back, "moved backward: {start:?} -> {:?}", sim.truth);
    }

    #[test]
    fn speed_ramps_gently_toward_target() {
        let mut sim = sim_at(0, 0.1);
        sim.speed = 0.0;
        let mut cmd = drive("schachbrett");
        cmd["accel"] = json!(0.5);
        send(&mut sim, &cmd);
        sim.tick(40.0, T0 + 40);
        assert!(
            sim.speed > 0.0 && sim.speed < 0.1,
            "ramping, not jumping: {}",
            sim.speed
        );
        run(&mut sim, 2_000);
        assert!((sim.speed - 0.6).abs() < 1e-9, "reached start speed");
    }

    #[test]
    fn ttl_expired_command_is_rejected() {
        let mut sim = sim_at(0, 0.1);
        let mut cmd = drive("bar");
        cmd["ts_ms"] = json!(T0 - 120_000);
        let out = send(&mut sim, &cmd);
        assert_eq!(exec_kinds(&out), vec!["expired"]);
        assert_eq!(sim.state, TrainState::Idle);
        assert!(sim.active.is_none());
    }

    #[test]
    fn duplicate_msg_id_is_ignored() {
        let mut sim = sim_at(0, 0.1);
        let cmd = drive("schachbrett");
        assert_eq!(exec_kinds(&send(&mut sim, &cmd)), vec!["accepted"]);
        assert!(exec_kinds(&send(&mut sim, &cmd)).is_empty());
    }

    #[test]
    fn cancel_and_replace_uses_old_cmd_id() {
        let mut sim = sim_at(0, 0.1);
        send(&mut sim, &drive("schachbrett"));
        let old_cmd_id = sim.active.as_ref().unwrap().cmd_id();

        let out = send(&mut sim, &drive("bar"));
        assert_eq!(exec_kinds(&out), vec!["cancelled", "accepted"]);
        let cancelled_cmd_id = out
            .iter()
            .find_map(|o| match o {
                SimOutput::Exec(e) if matches!(e.kind, ExecKind::Cancelled) => {
                    Some(e.envelope.cmd_id)
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(cancelled_cmd_id, old_cmd_id);
    }

    #[test]
    fn stop_and_raw_zero_cancel_with_active_cmd_id() {
        for stop_payload in [
            json!({
                "msg_id": Uuid::new_v4().to_string(), "cmd_id": Uuid::new_v4().to_string(),
                "seq": 9, "ts_ms": T0, "cmd_type": "stop"
            })
            .to_string()
            .into_bytes(),
            b"0".to_vec(),
        ] {
            let mut sim = sim_at(0, 0.1);
            send(&mut sim, &drive("schachbrett"));
            let active_cmd_id = sim.active.as_ref().unwrap().cmd_id();

            let out = sim.handle_payload(&stop_payload, T0);
            assert_eq!(exec_kinds(&out), vec!["cancelled"]);
            let cancelled = out
                .iter()
                .find_map(|o| match o {
                    SimOutput::Exec(e) => Some(e.envelope.cmd_id),
                    _ => None,
                })
                .unwrap();
            assert_eq!(cancelled, active_cmd_id);
            assert_eq!(sim.state, TrainState::Idle);
            assert_eq!(sim.motor, MotorMode::Stopped);
        }
    }

    #[test]
    fn rfid_timeout_safe_stops() {
        let mut sim = sim_at(0, 0.1);
        let mut cmd = drive("schachbrett");
        cmd["max_run_ms_without_tag"] = json!(1000);
        send(&mut sim, &cmd);
        let out = run(&mut sim, 3_000);
        assert_eq!(exec_kinds(&out), vec!["failed:RFID_TIMEOUT"]);
        assert_eq!(sim.state, TrainState::SafeStop);
        assert!(sim.active.is_none());
    }

    #[test]
    fn raw_commands_move_without_exec_events() {
        let mut sim = sim_at(0, 0.1);
        let out = sim.handle_payload(b"1", T0);
        assert!(exec_kinds(&out).is_empty());
        let start = sim.truth;
        let out = run(&mut sim, 6_000);
        assert!(exec_kinds(&out).is_empty(), "raw moves emit no exec events");
        assert!(
            sim.truth.frac > start.frac || sim.truth.edge != start.edge,
            "moved forward"
        );
        assert_eq!(sim.motor, MotorMode::Stopped, "steps exhausted");
        assert_eq!(sim.state, TrainState::Idle);

        let out = sim.handle_payload(b"3", T0);
        assert!(exec_kinds(&out).is_empty());
        let out = run(&mut sim, 10_000);
        assert!(
            out.iter().any(|o| matches!(o, SimOutput::Tag(_))),
            "raw moves still publish raw tag reads (setup aid)"
        );
        assert_eq!(sim.motor, MotorMode::RawForward, "keeps running");
        assert_eq!(sim.state, TrainState::Running);
    }

    #[test]
    fn raw_backward_reverses_and_tracks_belief() {
        // localize with a forward ride to bar
        let mut sim = sim_at(2, 0.5);
        send(&mut sim, &drive("bar"));
        run(&mut sim, 60_000);
        assert_eq!(sim.state, TrainState::Idle);
        let localized_edge = match sim.localizer.belief {
            Belief::Localized { edge } => edge,
            ref other => panic!("expected localized, got {other:?}"),
        };
        let start = sim.truth;

        // manual backward: moves down the ring, no exec events, belief tracks
        let out = sim.handle_payload(b"2", T0);
        assert!(exec_kinds(&out).is_empty());
        let out = run(&mut sim, 8_000);
        assert!(exec_kinds(&out).is_empty(), "backward emits no exec events");
        assert_eq!(sim.motor, MotorMode::RawBackward);
        let moved_back = sim.truth.edge != start.edge || sim.truth.frac < start.frac;
        assert!(moved_back, "moved backward: {start:?} -> {:?}", sim.truth);
        match sim.localizer.belief {
            Belief::Localized { edge } => {
                assert_eq!(edge, sim.truth.edge, "belief tracks truth while backing up");
                assert_ne!(edge, localized_edge);
            }
            ref other => panic!("expected still localized, got {other:?}"),
        }
    }

    #[test]
    fn garbage_payload_is_logged_not_fatal() {
        let mut sim = sim_at(0, 0.1);
        for garbage in [b"Alive".as_slice(), b"", b"{\"cmd_type\":\"fly\"}"] {
            let out = sim.handle_payload(garbage, T0);
            assert!(exec_kinds(&out).is_empty());
            assert!(matches!(out[0], SimOutput::Log(_)));
        }
        let out = sim.handle_track(b"nope");
        assert!(matches!(out[0], SimOutput::Log(_)));
        assert!(sim.table.is_some(), "a bad table keeps the old one");
    }

    #[test]
    fn heartbeat_reports_state_and_uptime() {
        let mut sim = sim_at(0, 0.1);
        let SimOutput::Status(status) = sim.heartbeat(T0 + 1_500) else {
            panic!()
        };
        assert_eq!(status.state, TrainState::Idle);
        assert_eq!(status.battery_pct, 86);
        assert_eq!(status.status_uptime_ms, 1_500);
        assert_eq!(status.envelope.cmd_id, CmdId::None);
    }
}
