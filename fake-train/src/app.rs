use std::collections::VecDeque;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use rumqttc::{AsyncClient, QoS};
use shotexpress_protocol::{ExecKind, TOPIC_EVENT_EXEC, TOPIC_STATUS};
use tokio::sync::mpsc;

use crate::mqtt::MqttIn;
use crate::sim::{SimOutput, Simulator};
use crate::ui::{self, UiState};

const TICK_MS: u64 = 40;
const LOG_CAPACITY: usize = 500;
const SPEED_STEPS: [f64; 6] = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0];

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
        }
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
                    broker: &self.broker,
                    paused: self.paused,
                    sim_speed: SPEED_STEPS[self.speed_step],
                    log: &self.log,
                };
                ui::draw(frame, &state);
            })?;

            tokio::select! {
                _ = tick.tick() => {
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
                let outputs = self.sim.handle_payload(&payload, now_ms());
                self.dispatch(outputs);
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
            }
        }
    }

    fn publish(&mut self, output: &SimOutput) {
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
        ExecKind::Progress { progress, .. } => match progress {
            Some(p) => format!(
                "progress last_tag={} distance={:.1} m",
                p.last_tag.as_deref().unwrap_or("?"),
                p.distance_m.unwrap_or(0.0)
            ),
            None => "progress".to_string(),
        },
        ExecKind::Completed => "completed".to_string(),
        ExecKind::Failed { error } => format!("failed {}: {}", error.code, error.reason),
        ExecKind::Cancelled => "cancelled".to_string(),
        ExecKind::Expired => "expired".to_string(),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_millis() as u64
}
