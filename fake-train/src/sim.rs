//! Headless simulation core: physics, train state machine and MQTT command
//! semantics. No tokio, no network — fully driveable from unit tests with a
//! synthetic clock. The behavioral reference is webserver/lib/fakeExpress.ts
//! plus the raw-integer protocol of the real Train.ino firmware.

use std::collections::HashSet;

use rand::RngExt;
use shotexpress_protocol::{
    CmdId, CommandKind, CommandMessage, Direction, Envelope, ExecError, ExecEvent, ExecKind,
    MoveCommandParameters, ProgressDetails, StatusMessage, TrainState,
};
use uuid::Uuid;

use crate::belief::{LocalizeOutcome, Localizer};
use crate::track::{self, CRAWL_SPEED, EDGES, NODES, READINGS, RingPos};

const DWELL_MS: f64 = 2_000.0;
const RAW_STEPS_MS: f64 = 2_000.0; // "1000 steps" at RAW_STEPS_SPEED
const RAW_STEPS_SPEED: f64 = 0.4;
const RAW_CONTINUOUS_SPEED: f64 = 0.6;
const MIN_SPEED: f64 = 0.05;

#[derive(Debug)]
pub enum SimOutput {
    Exec(ExecEvent),
    Status(StatusMessage),
    Log(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MotorMode {
    Stopped,
    Json,
    RawSteps { remaining_ms: f64 },
    RawContinuous,
}

#[derive(Debug)]
pub struct ActiveCommand {
    pub cmd_id: CmdId,
    pub target_tag: &'static str,
    pub ms_since_last_tag: f64,
    pub max_run_ms_without_tag: u64,
    pub distance_m: f64,
}

pub struct Simulator {
    pub truth: RingPos,
    pub direction: Direction,
    pub motor: MotorMode,
    pub speed: f64,
    pub state: TrainState,
    pub localizer: Localizer,
    pub loaded: bool,
    pub battery_pct: u8,
    pub active: Option<ActiveCommand>,
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
            direction: if rng.random_bool(0.5) {
                Direction::Forward
            } else {
                Direction::Reverse
            },
            motor: MotorMode::Stopped,
            speed: 0.0,
            state: TrainState::Idle,
            localizer: Localizer::new(),
            loaded: true,
            battery_pct: 86,
            active: None,
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
            // raw integer protocol of the current firmware
            "0" => return self.handle_raw_stop(now_ms),
            "1" => return self.handle_raw_steps(now_ms),
            "2" => return self.handle_raw_continuous(now_ms),
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
            CommandKind::MoveTo { ref params, ttl_ms } => {
                self.handle_move_to(&command.envelope, params, ttl_ms, now_ms)
            }
            CommandKind::Stop => {
                let mut out = vec![SimOutput::Log("cmd stop".into())];
                out.extend(self.stop_request(now_ms));
                out
            }
            ref other => {
                let name = other.name();
                vec![
                    SimOutput::Log(format!("cmd {name} -> unsupported")),
                    SimOutput::Exec(self.exec(
                        command.envelope.cmd_id,
                        ExecKind::Failed {
                            error: ExecError {
                                code: "UNSUPPORTED_COMMAND".into(),
                                reason: format!("Command {name} is not supported by fake-train"),
                            },
                        },
                        now_ms,
                    )),
                ]
            }
        }
    }

    fn handle_move_to(
        &mut self,
        envelope: &Envelope,
        params: &MoveCommandParameters,
        ttl_ms: u64,
        now_ms: u64,
    ) -> Vec<SimOutput> {
        let mut out = vec![SimOutput::Log(format!(
            "cmd move_to {} speed={} dir={:?} ttl={}ms",
            params.target.as_str(),
            params.speed,
            params.direction,
            ttl_ms
        ))];

        if now_ms > envelope.ts_ms.saturating_add(ttl_ms) {
            out.push(SimOutput::Exec(self.exec(
                envelope.cmd_id,
                ExecKind::Expired,
                now_ms,
            )));
            return out;
        }

        let Some(target_tag) = track::station_tag(params.target) else {
            out.push(SimOutput::Exec(self.exec(
                envelope.cmd_id,
                ExecKind::Failed {
                    error: ExecError {
                        code: "UNROUTABLE".into(),
                        reason: format!("no route to target {}", params.target.as_str()),
                    },
                },
                now_ms,
            )));
            return out;
        };

        // cancel-and-replace, mirroring fakeExpress clearScheduled()
        if let Some(active) = self.active.take() {
            out.push(SimOutput::Exec(self.exec(
                active.cmd_id,
                ExecKind::Cancelled,
                now_ms,
            )));
        }

        out.push(SimOutput::Exec(self.exec(
            envelope.cmd_id,
            ExecKind::Accepted,
            now_ms,
        )));
        self.set_direction(params.direction);
        self.speed = params.speed.clamp(MIN_SPEED, 1.0);
        self.motor = MotorMode::Json;
        self.active = Some(ActiveCommand {
            cmd_id: envelope.cmd_id,
            target_tag,
            ms_since_last_tag: 0.0,
            max_run_ms_without_tag: params.max_run_ms_without_tag,
            distance_m: 0.0,
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
        // firmware: move(1000) steps forward; no exec events (no cmd_id)
        let mut out = vec![SimOutput::Log("raw 1 -> move 1000 steps".into())];
        out.extend(self.cancel_active(now_ms));
        self.set_direction(Direction::Forward);
        self.speed = RAW_STEPS_SPEED;
        self.motor = MotorMode::RawSteps {
            remaining_ms: RAW_STEPS_MS,
        };
        self.set_state(TrainState::Running, now_ms, &mut out);
        out
    }

    fn handle_raw_continuous(&mut self, now_ms: u64) -> Vec<SimOutput> {
        // firmware: runBackward(), continuous; no exec events
        let mut out = vec![SimOutput::Log("raw 2 -> run continuous".into())];
        out.extend(self.cancel_active(now_ms));
        self.set_direction(Direction::Reverse);
        self.speed = RAW_CONTINUOUS_SPEED;
        self.motor = MotorMode::RawContinuous;
        self.set_state(TrainState::Running, now_ms, &mut out);
        out
    }

    pub fn stop_request(&mut self, now_ms: u64) -> Vec<SimOutput> {
        let mut out = self.cancel_active(now_ms);
        self.motor = MotorMode::Stopped;
        self.dwell_remaining_ms = 0.0;
        self.set_state(TrainState::Idle, now_ms, &mut out);
        out
    }

    fn cancel_active(&mut self, now_ms: u64) -> Vec<SimOutput> {
        match self.active.take() {
            // the Cancelled event must carry the *active* command's cmd_id,
            // otherwise the server's cmd_id filter drops it and stays busy
            Some(active) => vec![SimOutput::Exec(self.exec(
                active.cmd_id,
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
        self.direction = if rng.random_bool(0.5) {
            Direction::Forward
        } else {
            Direction::Reverse
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

        let move_ms = match self.motor {
            MotorMode::Stopped => 0.0,
            MotorMode::RawSteps { remaining_ms } => dt_ms.min(remaining_ms),
            _ => dt_ms,
        };

        let crossed = if move_ms > 0.0 {
            self.advance(move_ms, now_ms, &mut out)
        } else {
            false
        };

        if let MotorMode::RawSteps { remaining_ms } = &mut self.motor {
            *remaining_ms -= move_ms;
            if *remaining_ms <= 0.0 {
                self.motor = MotorMode::Stopped;
                self.set_state(TrainState::Idle, now_ms, &mut out);
            }
        }

        if let Some(active) = &mut self.active {
            if crossed {
                active.ms_since_last_tag = 0.0;
            } else {
                active.ms_since_last_tag += dt_ms;
                if active.ms_since_last_tag > active.max_run_ms_without_tag as f64 {
                    let cmd_id = active.cmd_id;
                    let max = active.max_run_ms_without_tag;
                    self.active = None;
                    self.motor = MotorMode::Stopped;
                    self.set_state(TrainState::SafeStop, now_ms, &mut out);
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

    /// Move for `move_ms`, crossing nodes as they come. Returns whether at
    /// least one tag was read.
    fn advance(&mut self, move_ms: f64, now_ms: u64, out: &mut Vec<SimOutput>) -> bool {
        let mut remaining = move_ms;
        let mut crossed = false;

        while remaining > 0.0 {
            let edge = EDGES[self.truth.edge];
            let edge_ms = edge.nominal_ms as f64 / self.effective_speed(edge.crawl);
            let time_to_node = match self.direction {
                Direction::Forward => (1.0 - self.truth.frac) * edge_ms,
                Direction::Reverse => self.truth.frac * edge_ms,
            };

            if remaining < time_to_node {
                let delta = remaining / edge_ms;
                match self.direction {
                    Direction::Forward => self.truth.frac += delta,
                    Direction::Reverse => self.truth.frac -= delta,
                }
                break;
            }

            remaining -= time_to_node;
            let node = track::next_node(self.truth.edge, self.direction);
            let traversed_len = edge.len_m;
            self.truth.edge = track::edge_after_crossing(node, self.direction);
            self.truth.frac = match self.direction {
                Direction::Forward => 0.0,
                Direction::Reverse => 1.0,
            };
            crossed = true;

            let stop = self.on_tag(node, traversed_len, now_ms, out);
            if stop {
                break;
            }
        }

        crossed
    }

    /// Handle a tag read at `node`. Returns true when the train stopped there.
    fn on_tag(
        &mut self,
        node: usize,
        traversed_len: f64,
        now_ms: u64,
        out: &mut Vec<SimOutput>,
    ) -> bool {
        let tag = READINGS[node];

        match self.localizer.on_reading(tag, self.direction) {
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

        let Some(active) = &mut self.active else {
            return false;
        };

        active.ms_since_last_tag = 0.0;
        active.distance_m += traversed_len;
        let distance_m = active.distance_m;
        let cmd_id = active.cmd_id;
        let target_tag = active.target_tag;

        if tag == target_tag {
            // arrival: fakeExpress order is status(DWELL) first, then completed
            self.motor = MotorMode::Stopped;
            self.active = None;
            self.dwell_remaining_ms = DWELL_MS;
            self.set_state(TrainState::Dwell, now_ms, out);
            out.push(SimOutput::Exec(self.exec(
                cmd_id,
                ExecKind::Completed,
                now_ms,
            )));
            return true;
        }

        // slow down once we know the next reading is the target
        if self.state == TrainState::Running
            && self.localizer.predicted_next(self.direction) == Some(target_tag)
        {
            self.set_state(TrainState::Approach, now_ms, out);
        }

        let direction = self.direction;
        out.push(SimOutput::Exec(self.exec(
            cmd_id,
            ExecKind::Progress {
                direction,
                progress: Some(ProgressDetails {
                    last_tag: Some(tag.to_string()),
                    distance_m: Some((distance_m * 10.0).round() / 10.0),
                }),
            },
            now_ms,
        )));

        false
    }

    // ---- outgoing messages ----

    pub fn heartbeat(&mut self, now_ms: u64) -> SimOutput {
        let cmd_id = self
            .active
            .as_ref()
            .map(|a| a.cmd_id)
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
        // fakeExpress publishes a status immediately on every state change
        out.push(self.heartbeat(now_ms));
    }

    fn set_direction(&mut self, direction: Direction) {
        if self.direction != direction {
            self.direction = direction;
            self.localizer.on_direction_change();
        }
    }

    fn effective_speed(&self, crawl_edge: bool) -> f64 {
        let mut speed = self.speed.max(MIN_SPEED);
        if crawl_edge || self.state == TrainState::Approach {
            speed = speed.min(CRAWL_SPEED);
        }
        speed
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

    fn sim_at(edge: usize, frac: f64, direction: Direction) -> Simulator {
        let mut sim = Simulator::new(&mut rand::rng(), T0);
        sim.truth = RingPos { edge, frac };
        sim.direction = direction;
        sim
    }

    fn move_to_payload(target: &str, direction: &str, ts_ms: u64, ttl_ms: u64) -> Vec<u8> {
        json!({
            "msg_id": Uuid::new_v4().to_string(),
            "cmd_id": Uuid::new_v4().to_string(),
            "seq": 1, "ts_ms": ts_ms,
            "cmd_type": "move_to",
            "params": {
                "target": target, "speed": 0.6, "direction": direction,
                "expected_tags": [], "stop_on_tag": "ignored",
                "max_run_ms_without_tag": 7000
            },
            "ttl_ms": ttl_ms
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
    fn happy_path_move_to_station() {
        // start just past station1 heading east; target station2
        let mut sim = sim_at(0, 0.1, Direction::Forward);
        let out = sim.handle_payload(&move_to_payload("schachbrett", "forward", T0, 60_000), T0);
        assert_eq!(exec_kinds(&out), vec!["accepted"]);
        assert_eq!(sim.state, TrainState::Running);

        let out = run(&mut sim, 30_000);
        let kinds = exec_kinds(&out);
        // track12 progress (with APPROACH once next==station2), then arrival
        assert_eq!(kinds, vec!["progress", "completed"]);
        assert_eq!(sim.state, TrainState::Idle, "back to IDLE after dwell");
        assert!(sim.active.is_none());
        // distances are monotone and rounded to 0.1
        let progress_distance = out.iter().find_map(|o| match o {
            SimOutput::Exec(ExecEvent {
                kind:
                    ExecKind::Progress {
                        progress: Some(p), ..
                    },
                ..
            }) => p.distance_m,
            _ => None,
        });
        assert_eq!(progress_distance, Some(1.2));
    }

    #[test]
    fn approach_state_before_target() {
        // long route so the belief localizes (track12 + station2) before the
        // tag ahead of the target: APPROACH requires knowing where you are
        let mut sim = sim_at(0, 0.1, Direction::Forward);
        sim.handle_payload(&move_to_payload("raucherecke", "forward", T0, 60_000), T0);
        let out = run(&mut sim, 30_000);
        assert!(
            out.iter()
                .any(|o| matches!(o, SimOutput::Status(s) if s.state == TrainState::Approach))
        );
        assert_eq!(
            exec_kinds(&out).last().map(String::as_str),
            Some("completed")
        );
    }

    #[test]
    fn move_completes_across_the_turnaround() {
        // just past station3 eastbound (edge 4 is station3->curveB1); target
        // station3 means: through loop B and back to node 7
        let mut sim = sim_at(4, 0.05, Direction::Forward);
        sim.handle_payload(&move_to_payload("raucherecke", "forward", T0, 60_000), T0);
        let out = run(&mut sim, 40_000);
        let kinds = exec_kinds(&out);
        assert_eq!(kinds, vec!["progress", "progress", "completed"]); // curveB1, curveB2, station3
        assert_eq!(sim.truth.frac, 0.0);
        assert_eq!(sim.truth.edge, 7);
    }

    #[test]
    fn unknown_spawn_localizes_after_two_tags() {
        let mut sim = sim_at(2, 0.5, Direction::Forward);
        assert_eq!(sim.localizer.belief, Belief::Unknown);
        sim.handle_payload(&move_to_payload("bar", "forward", T0, 60_000), T0);
        run(&mut sim, 3_000); // crosses track23 (node 3)
        assert!(matches!(sim.localizer.belief, Belief::Candidates { .. }));
        run(&mut sim, 4_000); // crosses station3 (node 4)
        assert_eq!(sim.localizer.belief, Belief::Localized { edge: 4 });
    }

    #[test]
    fn reverse_run_localizes_and_completes() {
        // on edge 2 (station2 -> track23) reversing: first reading station2
        let mut sim = sim_at(2, 0.9, Direction::Reverse);
        sim.handle_payload(&move_to_payload("bar", "reverse", T0, 60_000), T0);
        let out = run(&mut sim, 30_000);
        let kinds = exec_kinds(&out);
        // station2, track12, then station1 = target of "bar"
        assert_eq!(kinds, vec!["progress", "progress", "completed"]);
        assert!(matches!(sim.localizer.belief, Belief::Localized { .. }));
    }

    #[test]
    fn ttl_expired_command_is_rejected() {
        let mut sim = sim_at(0, 0.1, Direction::Forward);
        let stale = move_to_payload("schachbrett", "forward", T0 - 120_000, 60_000);
        let out = sim.handle_payload(&stale, T0);
        assert_eq!(exec_kinds(&out), vec!["expired"]);
        assert_eq!(sim.state, TrainState::Idle);
        assert!(sim.active.is_none());
    }

    #[test]
    fn forgot_name_is_unroutable() {
        let mut sim = sim_at(0, 0.1, Direction::Forward);
        let out = sim.handle_payload(&move_to_payload("forgot_name", "forward", T0, 60_000), T0);
        assert_eq!(exec_kinds(&out), vec!["failed:UNROUTABLE"]);
        assert_eq!(sim.state, TrainState::Idle);
    }

    #[test]
    fn wait_for_load_is_unsupported() {
        let mut sim = sim_at(0, 0.1, Direction::Forward);
        let payload = json!({
            "msg_id": Uuid::new_v4().to_string(), "cmd_id": Uuid::new_v4().to_string(),
            "seq": 1, "ts_ms": T0, "cmd_type": "wait_for_load",
            "wait_time_ms": 5000, "ttl_ms": 60000
        })
        .to_string();
        let out = sim.handle_payload(payload.as_bytes(), T0);
        assert_eq!(exec_kinds(&out), vec!["failed:UNSUPPORTED_COMMAND"]);
    }

    #[test]
    fn duplicate_msg_id_is_ignored() {
        let mut sim = sim_at(0, 0.1, Direction::Forward);
        let payload = move_to_payload("schachbrett", "forward", T0, 60_000);
        let first = sim.handle_payload(&payload, T0);
        assert_eq!(exec_kinds(&first), vec!["accepted"]);
        let second = sim.handle_payload(&payload, T0);
        assert!(exec_kinds(&second).is_empty());
    }

    #[test]
    fn cancel_and_replace_uses_old_cmd_id() {
        let mut sim = sim_at(0, 0.1, Direction::Forward);
        sim.handle_payload(&move_to_payload("raucherecke", "forward", T0, 60_000), T0);
        let old_cmd_id = sim.active.as_ref().unwrap().cmd_id;

        let out = sim.handle_payload(&move_to_payload("schachbrett", "forward", T0, 60_000), T0);
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
            let mut sim = sim_at(0, 0.1, Direction::Forward);
            sim.handle_payload(&move_to_payload("raucherecke", "forward", T0, 60_000), T0);
            let active_cmd_id = sim.active.as_ref().unwrap().cmd_id;

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
        let mut sim = sim_at(0, 0.1, Direction::Forward);
        let mut payload = move_to_payload("schachbrett", "forward", T0, 60_000);
        // shrink the timeout below the first tag-gap so it must trip
        let text = String::from_utf8(payload).unwrap().replace("7000", "1000");
        payload = text.into_bytes();
        sim.handle_payload(&payload, T0);
        let out = run(&mut sim, 3_000);
        assert_eq!(exec_kinds(&out), vec!["failed:RFID_TIMEOUT"]);
        assert_eq!(sim.state, TrainState::SafeStop);
        assert!(sim.active.is_none());
    }

    #[test]
    fn raw_commands_move_without_exec_events() {
        let mut sim = sim_at(0, 0.1, Direction::Reverse);
        let out = sim.handle_payload(b"1", T0);
        assert!(exec_kinds(&out).is_empty());
        assert_eq!(sim.direction, Direction::Forward);
        let start = sim.truth;
        let out = run(&mut sim, 4_000);
        assert!(exec_kinds(&out).is_empty(), "raw moves emit no exec events");
        assert_ne!(sim.truth, start);
        assert_eq!(sim.motor, MotorMode::Stopped, "steps exhausted");
        assert_eq!(sim.state, TrainState::Idle);

        let out = sim.handle_payload(b"2", T0);
        assert!(exec_kinds(&out).is_empty());
        assert_eq!(sim.direction, Direction::Reverse);
        run(&mut sim, 10_000);
        assert_eq!(sim.motor, MotorMode::RawContinuous, "keeps running");
        assert_eq!(sim.state, TrainState::Running);
    }

    #[test]
    fn garbage_payload_is_logged_not_fatal() {
        let mut sim = sim_at(0, 0.1, Direction::Forward);
        for garbage in [b"Alive".as_slice(), b"", b"{\"cmd_type\":\"fly\"}"] {
            let out = sim.handle_payload(garbage, T0);
            assert!(exec_kinds(&out).is_empty());
            assert!(matches!(out[0], SimOutput::Log(_)));
        }
    }

    #[test]
    fn heartbeat_reports_state_and_uptime() {
        let mut sim = sim_at(0, 0.1, Direction::Forward);
        let SimOutput::Status(status) = sim.heartbeat(T0 + 1_500) else {
            panic!()
        };
        assert_eq!(status.state, TrainState::Idle);
        assert_eq!(status.battery_pct, 86);
        assert_eq!(status.status_uptime_ms, 1_500);
        assert_eq!(status.envelope.cmd_id, CmdId::None);
    }
}
