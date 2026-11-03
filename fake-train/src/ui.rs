//! TUI rendering: static track art built once, with the train, belief
//! markers and panels overlaid per frame.

use std::collections::VecDeque;
use std::sync::LazyLock;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use shotexpress_protocol::TrainState;

use crate::belief::Belief;
use crate::sim::{Active, MotorMode, Simulator};
use crate::track::{self, NODES, READINGS, RingPos};

pub const TRACK_W: u16 = 100;
pub const TRACK_H: u16 = 7;
pub const MIN_WIDTH: u16 = TRACK_W + 4;
pub const MIN_HEIGHT: u16 = 22;

pub struct UiState<'a> {
    pub sim: &'a Simulator,
    pub connected: bool,
    /// Simulated wifi coverage of the train (false = out of coverage).
    pub signal: bool,
    pub broker: &'a str,
    pub paused: bool,
    pub sim_speed: f64,
    pub log: &'a VecDeque<(String, String)>,
}

// ---- track geometry ----
// Canvas is TRACK_W x TRACK_H. Main line on row 3 between the two loop
// boxes; loop traversal runs around the box perimeters. The westbound edges
// list the same main-line cells right-to-left, so main-line rendering is
// direction-agnostic (it is the same physical track).

const NODE_POS: [(u16, u16); NODES] = [
    (24, 3), // 0  station1
    (36, 3), // 1  track12
    (48, 3), // 2  station2
    (60, 3), // 3  track23
    (72, 3), // 4  station3
    (84, 1), // 5  curveB1
    (84, 5), // 6  curveB2
    (72, 3), // 7  station3 (westbound pass)
    (60, 3), // 8  track23
    (48, 3), // 9  station2
    (36, 3), // 10 track12
    (24, 3), // 11 station1
    (14, 5), // 12 curveA1
    (14, 1), // 13 curveA2
];

fn edge_waypoints(edge: usize) -> &'static [(u16, u16)] {
    match edge {
        0 => &[(24, 3), (36, 3)],
        1 => &[(36, 3), (48, 3)],
        2 => &[(48, 3), (60, 3)],
        3 => &[(60, 3), (72, 3)],
        4 => &[(72, 3), (82, 3), (82, 1), (84, 1)],
        5 => &[(84, 1), (96, 1), (96, 5), (84, 5)],
        6 => &[(84, 5), (82, 5), (82, 3), (72, 3)],
        7 => &[(72, 3), (60, 3)],
        8 => &[(60, 3), (48, 3)],
        9 => &[(48, 3), (36, 3)],
        10 => &[(36, 3), (24, 3)],
        11 => &[(24, 3), (16, 3), (16, 5), (14, 5)],
        12 => &[(14, 5), (2, 5), (2, 1), (14, 1)],
        13 => &[(14, 1), (16, 1), (16, 3), (24, 3)],
        _ => unreachable!(),
    }
}

/// Terminal cell for a fractional position along an edge polyline.
pub fn position_on_edge(edge: usize, frac: f64) -> (u16, u16) {
    let points = edge_waypoints(edge);
    let mut lengths = Vec::with_capacity(points.len() - 1);
    let mut total = 0.0;
    for pair in points.windows(2) {
        let dx = pair[1].0 as f64 - pair[0].0 as f64;
        let dy = (pair[1].1 as f64 - pair[0].1 as f64) * 2.0; // cells are taller than wide
        let len = (dx * dx + dy * dy).sqrt();
        lengths.push(len);
        total += len;
    }
    let mut remaining = frac.clamp(0.0, 1.0) * total;
    for (pair, len) in points.windows(2).zip(&lengths) {
        if remaining <= *len && *len > 0.0 {
            let t = remaining / len;
            let x = pair[0].0 as f64 + (pair[1].0 as f64 - pair[0].0 as f64) * t;
            let y = pair[0].1 as f64 + (pair[1].1 as f64 - pair[0].1 as f64) * t;
            return (x.round() as u16, y.round() as u16);
        }
        remaining -= len;
    }
    *points.last().unwrap()
}

enum ScreenHeading {
    East,
    West,
    Turning,
}

/// Which way the tip of the train points on screen. The train sits on the
/// ring facing forward, so this follows the ring direction at its position
/// regardless of whether it is momentarily rolling backwards.
fn screen_heading(pos: RingPos) -> ScreenHeading {
    let (x1, _) = position_on_edge(pos.edge, pos.frac);
    let (x2, _) = if pos.frac < 0.9 {
        position_on_edge(pos.edge, pos.frac + 0.1)
    } else {
        position_on_edge(track::next_node(pos.edge), 0.1)
    };
    match x2.cmp(&x1) {
        std::cmp::Ordering::Greater => ScreenHeading::East,
        std::cmp::Ordering::Less => ScreenHeading::West,
        std::cmp::Ordering::Equal => ScreenHeading::Turning,
    }
}

static TRACK_ART: LazyLock<Vec<String>> = LazyLock::new(build_track_art);

// range loops index two grid rows at once here; iterators would be noisier
#[allow(clippy::needless_range_loop)]
fn build_track_art() -> Vec<String> {
    let w = TRACK_W as usize;
    let mut grid = vec![vec![' '; w]; TRACK_H as usize];

    let mut draw_box = |x0: usize, x1: usize, y0: usize, y1: usize| {
        for x in (x0 + 1)..x1 {
            grid[y0][x] = '─';
            grid[y1][x] = '─';
        }
        for row in grid.iter_mut().take(y1).skip(y0 + 1) {
            row[x0] = '│';
            row[x1] = '│';
        }
        grid[y0][x0] = '╭';
        grid[y0][x1] = '╮';
        grid[y1][x0] = '╰';
        grid[y1][x1] = '╯';
    };
    draw_box(2, 16, 1, 5); // loop A
    draw_box(82, 96, 1, 5); // loop B

    for x in 17..82 {
        grid[3][x] = '─';
    }
    grid[3][16] = '├';
    grid[3][82] = '┤';

    let mut put = |x: usize, y: usize, text: &str| {
        for (i, c) in text.chars().enumerate() {
            if x + i < w {
                grid[y][x + i] = c;
            }
        }
    };
    // targets above their stations
    put(23, 0, "Bar");
    put(43, 0, "Schachbrett");
    put(67, 0, "Raucherecke");
    // tag names under the main line
    put(20, 4, "station1");
    put(33, 4, "track12");
    put(44, 4, "station2");
    put(57, 4, "track23");
    put(68, 4, "station3");
    // curve tags inside the loops
    put(4, 2, "curveA2");
    put(4, 4, "curveA1");
    put(86, 2, "curveB1");
    put(86, 4, "curveB2");

    for (x, y) in NODE_POS {
        grid[y as usize][x as usize] = '*';
    }

    grid.into_iter()
        .map(|row| row.into_iter().collect())
        .collect()
}

// ---- frame ----

pub fn draw(frame: &mut Frame, state: &UiState) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        frame.render_widget(
            Paragraph::new(format!(
                "terminal too small: need at least {MIN_WIDTH}x{MIN_HEIGHT}, got {}x{}",
                area.width, area.height
            )),
            area,
        );
        return;
    }

    let mqtt = if state.connected {
        Span::styled(
            format!(" MQTT connected {} ", state.broker),
            Style::default().fg(Color::Green),
        )
    } else {
        Span::styled(
            format!(" MQTT disconnected {} ", state.broker),
            Style::default().fg(Color::Red),
        )
    };
    let mut title = vec![Span::raw(" fake-train ─"), mqtt];
    if !state.signal {
        title.push(Span::styled(
            "─ 📵 NO SIGNAL ",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ));
    }
    let outer = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(title));
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let [track_area, panel_area, log_area, footer_area] = Layout::vertical([
        Constraint::Length(TRACK_H),
        Constraint::Length(6),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(inner);

    draw_track(frame, track_area, state);
    draw_panels(frame, panel_area, state);
    draw_log(frame, log_area, state);
    draw_footer(frame, footer_area, state);
}

fn draw_track(frame: &mut Frame, area: Rect, state: &UiState) {
    // center the fixed-size canvas; art and overlays share the same origin
    let track_rect = Rect {
        x: area.x + (area.width.saturating_sub(TRACK_W)) / 2,
        y: area.y,
        width: TRACK_W.min(area.width),
        height: TRACK_H.min(area.height),
    };
    let art: Vec<Line> = TRACK_ART
        .iter()
        .map(|row| Line::raw(row.as_str()))
        .collect();
    frame.render_widget(
        Paragraph::new(art).style(Style::default().fg(Color::DarkGray)),
        track_rect,
    );

    let buf = frame.buffer_mut();
    let cell = |x: u16, y: u16| (track_rect.x + x, track_rect.y + y);

    // belief overlay under the train
    match &state.sim.localizer.belief {
        Belief::Unknown => {}
        Belief::Candidates { edges, .. } => {
            for &edge in edges {
                let (x, y) = position_on_edge(edge, 0.5);
                let (bx, by) = cell(x, y);
                buf.set_string(
                    bx,
                    by,
                    "?",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                );
            }
        }
        Belief::Localized { edge } => {
            let (x, y) = position_on_edge(*edge, 0.5);
            let (bx, by) = cell(x, y);
            buf.set_string(
                bx,
                by,
                "▲",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            );
        }
    }

    // the train, topmost; the arrow marks the tip of the train, i.e. its
    // orientation on the ring — rolling backwards does not flip it
    let (x, y) = position_on_edge(state.sim.truth.edge, state.sim.truth.frac);
    let wagon = if state.sim.loaded { "[OOO]" } else { "[   ]" };
    let train = match screen_heading(state.sim.truth) {
        ScreenHeading::East => format!("{wagon}>"),
        ScreenHeading::West => format!("<{wagon}"),
        ScreenHeading::Turning => wagon.to_string(),
    };
    let half = (train.chars().count() / 2) as u16;
    let (tx, ty) = cell(
        x.saturating_sub(half)
            .min(TRACK_W - train.chars().count() as u16),
        y,
    );
    buf.set_string(
        tx,
        ty,
        &train,
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );
}

fn draw_panels(frame: &mut Frame, area: Rect, state: &UiState) {
    let [truth_area, belief_area, command_area] =
        Layout::horizontal([Constraint::Ratio(1, 3); 3]).areas(area);
    let sim = state.sim;

    let edge = sim.truth.edge;
    let from = READINGS[edge];
    let to = READINGS[(edge + 1) % NODES];
    let motor = match sim.motor {
        MotorMode::Stopped => "stopped".to_string(),
        MotorMode::Drive => format!("drive speed {:.2}", sim.speed),
        MotorMode::Steps {
            remaining_steps,
            backward,
        } => format!(
            "steps {} ({remaining_steps:.0} left)",
            if backward { "backward" } else { "forward" }
        ),
        MotorMode::RawForward => format!("raw forward speed {:.2}", sim.speed),
        MotorMode::RawBackward => format!("raw backward speed {:.2}", sim.speed),
    };
    let heading = match edge {
        0..=3 => "eastbound",
        4..=6 => "turnaround B",
        7..=10 => "westbound",
        _ => "turnaround A",
    };
    let truth = vec![
        Line::raw(format!(
            "edge  {from} → {to}  ({:.0}%)",
            sim.truth.frac * 100.0
        )),
        Line::raw(format!("track {heading}")),
        Line::raw(format!("motor {motor}")),
        Line::raw(format!(
            "state {}   battery {}%",
            sim.state.as_str(),
            sim.battery_pct
        )),
    ];
    frame.render_widget(
        Paragraph::new(truth).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Ground Truth "),
        ),
        truth_area,
    );

    let belief = match &sim.localizer.belief {
        Belief::Unknown => vec![Line::styled(
            "position unknown",
            Style::default().fg(Color::Yellow),
        )],
        Belief::Candidates { tag, edges } => vec![
            Line::styled(
                format!("{} candidates after tag {tag}", edges.len()),
                Style::default().fg(Color::Yellow),
            ),
            Line::raw("need one more tag to localize"),
        ],
        Belief::Localized { edge } => {
            let from = READINGS[*edge];
            let to = READINGS[(*edge + 1) % NODES];
            vec![
                Line::styled(
                    format!("localized: {from} → {to}"),
                    Style::default().fg(Color::Cyan),
                ),
                Line::raw(format!(
                    "next tag: {}",
                    sim.localizer.predicted_next().unwrap_or("?")
                )),
            ]
        }
    };
    frame.render_widget(
        Paragraph::new(belief).block(Block::default().borders(Borders::ALL).title(" Belief ")),
        belief_area,
    );

    let command = match &sim.active {
        Some(Active::Drive {
            target,
            target_uid,
            max_speed,
            ms_since_last_tag,
            max_run_ms_without_tag,
            distance_m,
            ..
        }) => vec![
            Line::raw(format!("drive → {target} (stop tag {target_uid})")),
            Line::raw(format!(
                "speed {:.2} → {:.2}  (zone {}, cap {max_speed:.2})",
                sim.speed,
                sim.target_speed,
                sim.zone_speed
                    .map(|z| format!("{z:.2}"))
                    .unwrap_or_else(|| "?".into()),
            )),
            Line::raw(format!(
                "no-tag {:.1}/{:.1} s   distance {distance_m:.1} m",
                ms_since_last_tag / 1000.0,
                *max_run_ms_without_tag as f64 / 1000.0
            )),
        ],
        Some(Active::Steps { .. }) => vec![Line::raw("steps nudge (positioning)")],
        None => vec![
            Line::raw("no active command"),
            Line::raw(format!(
                "shots: {}",
                if sim.loaded { "loaded" } else { "empty" }
            )),
        ],
    };
    let mut command = command;
    if sim.state == TrainState::SafeStop {
        command.push(Line::styled("SAFE_STOP", Style::default().fg(Color::Red)));
    }
    frame.render_widget(
        Paragraph::new(command).block(Block::default().borders(Borders::ALL).title(" Command ")),
        command_area,
    );
}

fn draw_log(frame: &mut Frame, area: Rect, state: &UiState) {
    let capacity = area.height.saturating_sub(2) as usize;
    let lines: Vec<Line> = state
        .log
        .iter()
        .rev()
        .take(capacity)
        .rev()
        .map(|(ts, msg)| {
            Line::from(vec![
                Span::styled(format!("{ts}  "), Style::default().fg(Color::DarkGray)),
                Span::raw(msg.as_str()),
            ])
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" Event Log ")),
        area,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect, state: &UiState) {
    let paused = if state.paused { "  ▌▌ PAUSED" } else { "" };
    let footer = format!(
        " q quit   space pause   e shots   r randomize   s signal   +/- sim speed (x{:.2}){paused}",
        state.sim_speed
    );
    frame.render_widget(
        Paragraph::new(footer).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// Renders a frame to text; run with `cargo test -p fake-train render -- --nocapture`
    /// to eyeball the layout.
    #[test]
    fn render_smoke_test() {
        let mut sim = Simulator::new(&mut rand::rng(), 0);
        sim.truth = crate::track::RingPos { edge: 1, frac: 0.5 };
        let mut log = VecDeque::new();
        log.push_back((
            "00:01.2".to_string(),
            "tag track12 -> 2 candidate positions".to_string(),
        ));
        log.push_back((
            "00:03.4".to_string(),
            "exec progress last_tag=track12".to_string(),
        ));

        let state = UiState {
            sim: &sim,
            connected: true,
            signal: true,
            broker: "mqtt://localhost:1883",
            paused: false,
            sim_speed: 1.0,
            log: &log,
        };

        let backend = TestBackend::new(110, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &state)).unwrap();

        let buffer = terminal.backend().buffer().clone();
        let mut text = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                text.push_str(buffer[(x, y)].symbol());
            }
            text.push('\n');
        }
        println!("{text}");
        assert!(text.contains("[OOO]>"));
        assert!(text.contains("Raucherecke"));
        assert!(text.contains("Event Log"));
    }

    #[test]
    fn positions_follow_the_loop() {
        // loop B perimeter: start at entry corner, end back at the bottom
        assert_eq!(position_on_edge(5, 0.0), (84, 1));
        assert_eq!(position_on_edge(5, 1.0), (84, 5));
        let (x, y) = position_on_edge(5, 0.5);
        assert!(
            x > 84 && (y == 1 || y == 5 || x == 96),
            "midway on the loop, got ({x},{y})"
        );
        // main line stays on row 3, westbound maps to the same cells
        assert_eq!(position_on_edge(0, 0.5), position_on_edge(10, 0.5));
    }
}
