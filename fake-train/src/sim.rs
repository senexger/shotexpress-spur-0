//! Headless simulation core: physics, train state machine and MQTT command
//! semantics. No tokio, no network — fully driveable from unit tests with a
//! synthetic clock. Behaves like the target Train.ino firmware contract:
//! a dumb executor for `drive` leg schedules and `steps` nudges.
//!
//! The train only ever runs forward on autonomous rides: the passive
//! switches route it around the turnaround loops, so "going back" means
//! continuing forward through the next turnaround. Backward motion exists
//! only for `steps` (refill positioning) and the raw manual "2".
//!
//! Failure reflexes, in order of preference: resync forward past missed
//! reads (harmless on a one-way ring), safe-stop and report when the world
//! contradicts the schedule (`OVERSHOT`, `UNEXPECTED_TAG`), safe-stop on
//! the RFID watchdog. Never improvise a new plan — that's the server's job.

use std::collections::HashSet;

use rand::RngExt;
use shotexpress_protocol::{
    CmdId, CommandKind, CommandMessage, Direction, Envelope, ExecError, ExecEvent, ExecKind, Leg,
    StatusMessage, TrainState,
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
    /// Executing a drive leg schedule (always forward).
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
        /// Remaining legs, front = current. The last one is the stop tag.
        legs: Vec<Leg>,
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

pub struct Simulator {
    pub truth: RingPos,
    pub motor: MotorMode,
    /// Current (ramped) speed; approaches `target_speed` at `accel`/s.
    pub speed: f64,
    pub target_speed: f64,
    pub accel: f64,
    pub state: TrainState,
    pub localizer: Localizer,
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
                legs,
                accel,
                max_run_ms_without_tag,
                ttl_ms,
            } => self.handle_drive(
                &command.envelope,
                legs,
                accel,
                max_run_ms_without_tag,
                ttl_ms,
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

    fn handle_drive(
        &mut self,
        envelope: &Envelope,
        legs: Vec<Leg>,
        accel: f64,
        max_run_ms_without_tag: u64,
        ttl_ms: u64,
        now_ms: u64,
    ) -> Vec<SimOutput> {
        let mut out = vec![SimOutput::Log(format!(
            "cmd drive {} leg(s), stop at {}, ttl={ttl_ms}ms",
            legs.len(),
            legs.last().map(|l| l.tag.as_str()).unwrap_or("-"),
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

        // empty schedule = "come to a stop": completes on the spot
        let Some(first) = legs.first() else {
            self.motor = MotorMode::Stopped;
            self.speed = 0.0;
            self.target_speed = 0.0;
            self.set_state(TrainState::Idle, now_ms, &mut out);
            out.push(SimOutput::Exec(self.exec(
                envelope.cmd_id,
                ExecKind::Completed,
                now_ms,
            )));
            return out;
        };

        self.accel = accel.max(0.01);
        self.target_speed = first.speed.clamp(MIN_SPEED, 1.0);
        self.motor = MotorMode::Drive;
        let approach = legs.len() == 1;
        self.active = Some(Active::Drive {
            cmd_id: envelope.cmd_id,
            legs,
            ms_since_last_tag: 0.0,
            max_run_ms_without_tag,
            distance_m: 0.0,
        });
        let state = if approach {
            TrainState::Approach
        } else {
            TrainState::Running
        };
        self.set_state(state, now_ms, &mut out);
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

        let Some(Active::Drive {
            cmd_id,
            legs,
            ms_since_last_tag,
            distance_m,
            ..
        }) = &mut self.active
        else {
            return (true, false);
        };
        *ms_since_last_tag = 0.0;
        let cmd_id = *cmd_id;
        let distance_m = (*distance_m * 10.0).round() / 10.0;

        // the local ruleset: match the read against the remaining schedule
        match legs.iter().position(|leg| leg.tag == tag) {
            Some(k) => {
                let skipped = k as u32;
                legs.drain(..=k);
                if legs.is_empty() {
                    // arrival (possibly resynced straight onto the stop tag)
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

                self.target_speed = legs[0].speed.clamp(MIN_SPEED, 1.0);
                let approach = legs.len() == 1;
                out.push(SimOutput::Exec(self.exec(
                    cmd_id,
                    ExecKind::Progress {
                        last_tag: tag.to_string(),
                        distance_m,
                        skipped,
                    },
                    now_ms,
                )));
                if approach && self.state == TrainState::Running {
                    self.set_state(TrainState::Approach, now_ms, out);
                }
                (true, false)
            }
            None => {
                // stupid-train heuristic: an unknown tag while only the stop
                // tag remained means we rolled past it
                let (code, reason) = if legs.len() == 1 {
                    (
                        "OVERSHOT",
                        format!("missed stop tag {}, read {tag} instead", legs[0].tag),
                    )
                } else {
                    (
                        "UNEXPECTED_TAG",
                        format!("tag {tag} is not in the remaining schedule"),
                    )
                };
                self.safe_stop(now_ms, out);
                out.push(SimOutput::Exec(self.exec(
                    cmd_id,
                    ExecKind::Failed {
                        error: ExecError {
                            code: code.into(),
                            reason,
                        },
                    },
                    now_ms,
                )));
                (true, true)
            }
        }
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
    use serde_json::json;

    const T0: u64 = 1_783_000_000_000;

    fn sim_at(edge: usize, frac: f64) -> Simulator {
        let mut sim = Simulator::new(&mut rand::rng(), T0);
        sim.truth = RingPos { edge, frac };
        // tests reason about tag gaps, not ramp physics: start rides at
        // cruise instantly unless a test opts back in
        sim.speed = 0.6;
        sim.accel = 1e9;
        sim
    }

    fn drive_payload(legs: &[(&str, f64)], ts_ms: u64, ttl_ms: u64) -> Vec<u8> {
        drive_payload_accel(legs, 1e9, ts_ms, ttl_ms)
    }

    fn drive_payload_accel(legs: &[(&str, f64)], accel: f64, ts_ms: u64, ttl_ms: u64) -> Vec<u8> {
        json!({
            "msg_id": Uuid::new_v4().to_string(),
            "cmd_id": Uuid::new_v4().to_string(),
            "seq": 1, "ts_ms": ts_ms,
            "cmd_type": "drive",
            "legs": legs.iter()
                .map(|(tag, speed)| json!({"tag": tag, "speed": speed}))
                .collect::<Vec<_>>(),
            "accel": accel,
            "max_run_ms_without_tag": 7000,
            "ttl_ms": ttl_ms
        })
        .to_string()
        .into_bytes()
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
                    ExecKind::Progress { skipped, .. } if *skipped > 0 => {
                        format!("progress+{skipped}")
                    }
                    ExecKind::Progress { .. } => "progress".to_string(),
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

    #[test]
    fn happy_path_drive_to_station() {
        // start just past station1 heading east; stop at station2
        let mut sim = sim_at(0, 0.1);
        let out = sim.handle_payload(
            &drive_payload(&[("track12", 0.6), ("station2", 0.35)], T0, 60_000),
            T0,
        );
        assert_eq!(exec_kinds(&out), vec!["accepted"]);
        assert_eq!(sim.state, TrainState::Running);

        let out = run(&mut sim, 30_000);
        let kinds = exec_kinds(&out);
        assert_eq!(kinds, vec!["progress", "completed"]);
        assert_eq!(sim.state, TrainState::Idle, "back to IDLE after dwell");
        assert!(sim.active.is_none());
        assert_eq!(sim.truth.edge, 2, "stopped on the station2 node");
        // distances are monotone and rounded to 0.1
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
    fn approach_state_on_final_leg() {
        let mut sim = sim_at(0, 0.1);
        sim.handle_payload(
            &drive_payload(&[("track12", 0.6), ("station2", 0.35)], T0, 60_000),
            T0,
        );
        let out = run(&mut sim, 4_000); // crosses track12, one leg left
        assert!(
            out.iter()
                .any(|o| matches!(o, SimOutput::Status(s) if s.state == TrainState::Approach)),
            "APPROACH once only the stop leg remains"
        );
        assert_eq!(sim.target_speed, 0.35, "crawling toward the stop tag");
    }

    #[test]
    fn legs_carry_the_speed_schedule() {
        let mut sim = sim_at(4, 0.05); // entering loop B
        // 0.5 on the curve leg mirrors track.toml's turnaround speed_limit:
        // 2500 ms / 0.5 = 5000 ms stays under the 7 s watchdog
        sim.handle_payload(
            &drive_payload(
                &[("curveB1", 0.8), ("curveB2", 0.5), ("station3", 0.35)],
                T0,
                60_000,
            ),
            T0,
        );
        run(&mut sim, 3_000); // crossed curveB1 -> curve leg active
        assert_eq!(sim.target_speed, 0.5, "slow through the turnaround");
        let out = run(&mut sim, 40_000);
        assert_eq!(
            exec_kinds(&out).last().map(String::as_str),
            Some("completed")
        );
        assert_eq!(sim.truth.edge, 7, "arrived at westbound station3 node");
    }

    #[test]
    fn missed_tag_resyncs_forward() {
        let mut sim = sim_at(0, 0.1);
        sim.miss_next_tags = 1; // track12 read fails
        sim.handle_payload(
            &drive_payload(
                &[("track12", 0.6), ("station2", 0.6), ("track23", 0.35)],
                T0,
                60_000,
            ),
            T0,
        );
        let out = run(&mut sim, 30_000);
        let kinds = exec_kinds(&out);
        // station2 arrives with skipped=1, then the stop tag
        assert_eq!(kinds, vec!["progress+1", "completed"]);
        assert_eq!(sim.state, TrainState::Idle);
    }

    #[test]
    fn missed_stop_tag_is_overshot() {
        // 0.6 keeps two blind edges under the 7 s watchdog, so the next
        // tag (station2) arrives and exposes the overshoot
        let mut sim = sim_at(0, 0.1);
        sim.miss_next_tags = 1; // the stop tag read fails
        sim.handle_payload(&drive_payload(&[("track12", 0.6)], T0, 60_000), T0);
        let out = run(&mut sim, 10_000);
        assert_eq!(exec_kinds(&out), vec!["failed:OVERSHOT"]);
        assert_eq!(sim.state, TrainState::SafeStop);
        assert!(sim.active.is_none());
        assert_eq!(sim.truth.edge, 2, "stopped one tag past the target");
    }

    #[test]
    fn foreign_tag_is_unexpected() {
        // schedule says loop B, but the train is on the eastbound main line
        let mut sim = sim_at(0, 0.1);
        sim.handle_payload(
            &drive_payload(&[("curveB1", 0.6), ("curveB2", 0.35)], T0, 60_000),
            T0,
        );
        let out = run(&mut sim, 10_000);
        assert_eq!(exec_kinds(&out), vec!["failed:UNEXPECTED_TAG"]);
        assert_eq!(sim.state, TrainState::SafeStop);
    }

    #[test]
    fn empty_legs_completes_on_the_spot() {
        let mut sim = sim_at(0, 0.1);
        sim.handle_payload(&drive_payload(&[("track12", 0.6)], T0, 60_000), T0);
        let out = sim.handle_payload(&drive_payload(&[], T0, 60_000), T0);
        assert_eq!(exec_kinds(&out), vec!["cancelled", "accepted", "completed"]);
        assert_eq!(sim.state, TrainState::Idle);
        assert_eq!(sim.motor, MotorMode::Stopped);
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
        let payload = drive_payload_accel(&[("track12", 0.6), ("station2", 0.6)], 0.5, T0, 60_000);
        sim.handle_payload(&payload, T0);
        sim.tick(40.0, T0 + 40);
        assert!(
            sim.speed > 0.0 && sim.speed < 0.1,
            "ramping, not jumping: {}",
            sim.speed
        );
        run(&mut sim, 2_000);
        assert!((sim.speed - 0.6).abs() < 1e-9, "reached cruise");
    }

    #[test]
    fn ttl_expired_command_is_rejected() {
        let mut sim = sim_at(0, 0.1);
        let stale = drive_payload(&[("track12", 0.6)], T0 - 120_000, 60_000);
        let out = sim.handle_payload(&stale, T0);
        assert_eq!(exec_kinds(&out), vec!["expired"]);
        assert_eq!(sim.state, TrainState::Idle);
        assert!(sim.active.is_none());
    }

    #[test]
    fn duplicate_msg_id_is_ignored() {
        let mut sim = sim_at(0, 0.1);
        let payload = drive_payload(&[("track12", 0.6), ("station2", 0.35)], T0, 60_000);
        let first = sim.handle_payload(&payload, T0);
        assert_eq!(exec_kinds(&first), vec!["accepted"]);
        let second = sim.handle_payload(&payload, T0);
        assert!(exec_kinds(&second).is_empty());
    }

    #[test]
    fn cancel_and_replace_uses_old_cmd_id() {
        let mut sim = sim_at(0, 0.1);
        sim.handle_payload(
            &drive_payload(&[("track12", 0.6), ("station2", 0.35)], T0, 60_000),
            T0,
        );
        let old_cmd_id = sim.active.as_ref().unwrap().cmd_id();

        let out = sim.handle_payload(&drive_payload(&[("track12", 0.35)], T0, 60_000), T0);
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
            sim.handle_payload(
                &drive_payload(&[("track12", 0.6), ("station2", 0.35)], T0, 60_000),
                T0,
            );
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
        let payload = drive_payload(&[("track12", 0.6), ("station2", 0.35)], T0, 60_000);
        let text = String::from_utf8(payload).unwrap().replace("7000", "1000");
        sim.handle_payload(text.as_bytes(), T0);
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
        // localize first with a forward ride to station1
        let mut sim = sim_at(2, 0.5);
        sim.handle_payload(
            &drive_payload(
                &[
                    ("track23", 0.6),
                    ("station3", 0.4),
                    ("curveB1", 0.6),
                    ("curveB2", 0.5),
                    ("station3", 0.4),
                    ("track23", 0.6),
                    ("station2", 0.4),
                    ("track12", 0.6),
                    ("station1", 0.35),
                ],
                T0,
                60_000,
            ),
            T0,
        );
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
        // belief followed the backward crossings
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
