use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use rand::RngExt;
use ratatui::DefaultTerminal;
use rumqttc::{AsyncClient, QoS};
use shotexpress_protocol::{ExecKind, TOPIC_EVENT_EXEC, TOPIC_EVENT_TAG, TOPIC_STATUS, now_ms};
use tokio::sync::mpsc;

use crate::mqtt::MqttIn;
use crate::sim::{SimOutput, Simulator};
use crate::ui::{self, UiState};

const TICK_MS: u64 = 40;
const LOG_CAPACITY: usize = 500;
const SPEED_STEPS: [f64; 6] = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0];

// Random wifi outages: the loops sit far from the access point, so coverage
// there is much worse. Durations are wall-clock (independent of sim speed).
const OUTAGE_MEAN_INTERVAL_S: f64 = 90.0;
const OUTAGE_MEAN_INTERVAL_TURNAROUND_S: f64 = 25.0;
const OUTAGE_MIN_S: f64 = 3.0;
const OUTAGE_MAX_S: f64 = 8.0;

pub struct App {
    sim: Simulator,
    client: AsyncClient,
    rx: mpsc::Receiver<MqttIn>,
    broker: String,
    connected: bool,
    paused: bool,
    speed_step: usize,
    log: VecDeque<(String, String)>,
    started: Instant,
    should_quit: bool,
    // simulated wifi coverage: while offline the train keeps driving, queues
    // its exec events (store-and-forward) and receives queued commands late
    // (as a broker with a persistent session would deliver them)
    manual_offline: bool,
    outage_until: Option<Instant>,
    was_online: bool,
    queued_events: Vec<String>,
    queued_commands: Vec<Vec<u8>>,
}

impl App {
    pub fn new(client: AsyncClient, rx: mpsc::Receiver<MqttIn>, broker: String) -> Self {
        App {
            sim: Simulator::new(&mut rand::rng(), now_ms()),
            client,
            rx,
            broker,
            connected: false,
            paused: false,
            speed_step: 2, // 1.0x
            log: VecDeque::new(),
            started: Instant::now(),
            should_quit: false,
            manual_offline: false,
            outage_until: None,
            was_online: false,
            queued_events: Vec::new(),
            queued_commands: Vec::new(),
        }
    }

    /// Simulated wifi coverage of the train (independent of the real MQTT
    /// connection of this process).
    fn signal(&self) -> bool {
        !self.manual_offline && self.outage_until.is_none()
    }

    fn online(&self) -> bool {
        self.connected && self.signal()
    }

    pub async fn run(mut self, mut terminal: DefaultTerminal) -> std::io::Result<()> {
        let mut tick = tokio::time::interval(Duration::from_millis(TICK_MS));
        let mut heartbeat = tokio::time::interval(Duration::from_secs(1));
        let mut term_events = EventStream::new();

        self.push_log("simulator started at a random position, belief unknown".into());

        while !self.should_quit {
            terminal.draw(|frame| {
                let state = UiState {
                    sim: &self.sim,
                    connected: self.connected,
                    signal: self.signal(),
                    broker: &self.broker,
                    paused: self.paused,
                    sim_speed: SPEED_STEPS[self.speed_step],
                    log: &self.log,
                };
                ui::draw(frame, &state);
            })?;

            tokio::select! {
                _ = tick.tick() => {
                    self.update_signal();
                    if !self.paused {
                        let dt = TICK_MS as f64 * SPEED_STEPS[self.speed_step];
                        let outputs = self.sim.tick(dt, now_ms());
                        self.dispatch(outputs);
                    }
                }
                _ = heartbeat.tick() => {
                    // keeps running while paused so the dashboard stays alive
                    let status = self.sim.heartbeat(now_ms());
                    self.publish(&status);
                }
                Some(Ok(event)) = term_events.next() => {
                    self.handle_terminal_event(event);
                }
                Some(msg) = self.rx.recv() => {
                    self.handle_mqtt(msg);
                }
            }
        }
        Ok(())
    }

    /// End expired outages, randomly start new ones, and flush the queues on
    /// the offline -> online transition.
    fn update_signal(&mut self) {
        if let Some(until) = self.outage_until
            && Instant::now() >= until
        {
            self.outage_until = None;
        }

        if self.signal() && !self.paused {
            let mut rng = rand::rng();
            let mean_s = if matches!(self.sim.truth.edge, 4..=6 | 11..=13) {
                OUTAGE_MEAN_INTERVAL_TURNAROUND_S
            } else {
                OUTAGE_MEAN_INTERVAL_S
            };
            let per_tick = (TICK_MS as f64 / 1000.0) / mean_s;
            if rng.random_bool(per_tick.clamp(0.0, 1.0)) {
                let duration = rng.random_range(OUTAGE_MIN_S..OUTAGE_MAX_S);
                self.outage_until = Some(Instant::now() + Duration::from_secs_f64(duration));
                self.push_log(format!("📵 signal lost (~{duration:.1} s, bad coverage)"));
            }
        }

        let online = self.online();
        if online && !self.was_online {
            self.flush_queues();
        }
        self.was_online = online;
    }

    fn flush_queues(&mut self) {
        if !self.queued_events.is_empty() || !self.queued_commands.is_empty() {
            self.push_log(format!(
                "signal restored: flushing {} queued event(s), {} pending command(s)",
                self.queued_events.len(),
                self.queued_commands.len()
            ));
        }
        // the train's report of what happened offline goes out first
        for payload in std::mem::take(&mut self.queued_events) {
            if let Err(err) =
                self.client
                    .try_publish(TOPIC_EVENT_EXEC, QoS::AtLeastOnce, false, payload)
            {
                self.push_log(format!("publish failed: {err}"));
            }
        }
        // then commands the broker held back are delivered late; stale ones
        // die on their TTL inside the sim
        for payload in std::mem::take(&mut self.queued_commands) {
            let outputs = self.sim.handle_payload(&payload, now_ms());
            self.dispatch(outputs);
        }
    }

    fn handle_terminal_event(&mut self, event: Event) {
        let Event::Key(key) = event else { return };
        if key.kind != KeyEventKind::Press {
            return;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            KeyCode::Char(' ') => {
                self.paused = !self.paused;
                let state = if self.paused { "paused" } else { "resumed" };
                self.push_log(format!("simulation {state}"));
            }
            KeyCode::Char('e') => {
                self.sim.loaded = !self.sim.loaded;
                let state = if self.sim.loaded { "loaded" } else { "empty" };
                self.push_log(format!("shots {state}"));
            }
            KeyCode::Char('r') => {
                let outputs = self.sim.randomize(&mut rand::rng(), now_ms());
                self.dispatch(outputs);
            }
            KeyCode::Char('s') => {
                self.manual_offline = !self.manual_offline;
                if self.manual_offline {
                    self.outage_until = None;
                    self.push_log("📵 signal forced off".into());
                } else {
                    self.push_log("signal forced back on".into());
                }
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.speed_step = (self.speed_step + 1).min(SPEED_STEPS.len() - 1);
            }
            KeyCode::Char('-') => {
                self.speed_step = self.speed_step.saturating_sub(1);
            }
            _ => {}
        }
    }

    fn handle_mqtt(&mut self, msg: MqttIn) {
        match msg {
            MqttIn::Connected => {
                self.connected = true;
                self.push_log("connected to MQTT broker".into());
            }
            MqttIn::Disconnected(reason) => {
                if self.connected {
                    self.push_log(format!("MQTT connection lost: {reason}"));
                }
                self.connected = false;
            }
            MqttIn::Command(payload) => {
                if self.signal() {
                    let outputs = self.sim.handle_payload(&payload, now_ms());
                    self.dispatch(outputs);
                } else {
                    // out of coverage: the broker holds the command and
                    // delivers it once the train reconnects
                    self.push_log("no signal: command held back until reconnect".into());
                    self.queued_commands.push(payload);
                }
            }
        }
    }

    fn dispatch(&mut self, outputs: Vec<SimOutput>) {
        for output in outputs {
            match &output {
                SimOutput::Log(msg) => self.push_log(msg.clone()),
                SimOutput::Status(status) => {
                    // non-heartbeat statuses are state transitions: log them
                    self.push_log(format!("state → {}", status.state.as_str()));
                    self.publish(&output);
                }
                SimOutput::Exec(event) => {
                    self.push_log(format!("exec {}", describe_exec(&event.kind)));
                    self.publish(&output);
                }
                // already logged by the sim's localizer trace
                SimOutput::Tag(_) => self.publish(&output),
            }
        }
    }

    fn publish(&mut self, output: &SimOutput) {
        if !self.online() {
            // store-and-forward for exec events (the server dedupes by
            // msg_id); statuses are transient and just dropped. This also
            // keeps the rumqttc request channel from backing up while the
            // real connection is down.
            if let SimOutput::Exec(event) = output {
                self.queued_events
                    .push(serde_json::to_string(event).expect("exec serialization cannot fail"));
            }
            return;
        }
        let result = match output {
            SimOutput::Exec(event) => self.client.try_publish(
                TOPIC_EVENT_EXEC,
                QoS::AtLeastOnce,
                false,
                serde_json::to_string(event).expect("exec event serialization cannot fail"),
            ),
            SimOutput::Status(status) => self.client.try_publish(
                TOPIC_STATUS,
                QoS::AtMostOnce,
                false,
                serde_json::to_string(status).expect("status serialization cannot fail"),
            ),
            // raw uid payload, transient like status: dropped while offline
            SimOutput::Tag(tag) => {
                self.client
                    .try_publish(TOPIC_EVENT_TAG, QoS::AtMostOnce, false, tag.as_str())
            }
            SimOutput::Log(_) => return,
        };
        if let Err(err) = result {
            self.push_log(format!("publish failed: {err}"));
        }
    }

    fn push_log(&mut self, msg: String) {
        let elapsed = self.started.elapsed();
        let ts = format!(
            "{:02}:{:02}.{}",
            elapsed.as_secs() / 60,
            elapsed.as_secs() % 60,
            elapsed.subsec_millis() / 100
        );
        if self.log.len() >= LOG_CAPACITY {
            self.log.pop_front();
        }
        self.log.push_back((ts, msg));
    }
}

fn describe_exec(kind: &ExecKind) -> String {
    match kind {
        ExecKind::Accepted => "accepted".to_string(),
        ExecKind::Progress {
            last_tag,
            distance_m,
            skipped,
        } => {
            if *skipped > 0 {
                format!(
                    "progress last_tag={last_tag} distance={distance_m:.1} m (skipped {skipped})"
                )
            } else {
                format!("progress last_tag={last_tag} distance={distance_m:.1} m")
            }
        }
        ExecKind::Completed => "completed".to_string(),
        ExecKind::Failed { error } => format!("failed {}: {}", error.code, error.reason),
        ExecKind::Cancelled => "cancelled".to_string(),
        ExecKind::Expired => "expired".to_string(),
    }
}
