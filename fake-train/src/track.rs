//! Track topology: a main line with a passive turnaround loop at each end
//! (dogbone). Flattened, this is a single ring of tag *readings*: eastbound
//! main line, loop B, westbound main line (same physical track, opposite
//! pass), loop A. Main-line tags therefore appear at two ring nodes, curve
//! tags at one.
//!
//! Motor `Direction::Forward` moves the ring coordinate up; crossing into
//! node `j` emits reading `READINGS[j]`. `RingPos { edge, frac }` is a point
//! on edge `edge`, which connects node `edge` to node `(edge + 1) % NODES`.

use shotexpress_protocol::{Direction, Target};

pub const NODES: usize = 14;

pub const READINGS: [&str; NODES] = [
    "station1", "track12", "station2", "track23", "station3", // 0..=4  eastbound main line
    "curveB1", "curveB2", // 5..=6  loop B (east turnaround)
    "station3", "track23", "station2", "track12", "station1", // 7..=11 westbound main line
    "curveA1", "curveA2", // 12..=13 loop A (west turnaround)
];

#[derive(Debug, Clone, Copy)]
pub struct Edge {
    pub nominal_ms: u64,
    pub len_m: f64,
    pub crawl: bool,
}

const MAIN: Edge = Edge {
    nominal_ms: 2_000,
    len_m: 1.2,
    crawl: false,
};
const LOOP_LINK: Edge = Edge {
    nominal_ms: 2_000,
    len_m: 1.5,
    crawl: false,
};
// The turnaround itself. Worst tag gap on the ring is this edge at crawl
// speed: 2500 / 0.5 = 5000 ms, which must stay below the default
// max_run_ms_without_tag of 7000 ms or every lap ends in SAFE_STOP.
const TURNAROUND: Edge = Edge {
    nominal_ms: 2_500,
    len_m: 2.0,
    crawl: true,
};

/// EDGES[i] connects node i -> node (i + 1) % NODES.
pub const EDGES: [Edge; NODES] = [
    MAIN,       // 0  station1 -> track12
    MAIN,       // 1  track12  -> station2
    MAIN,       // 2  station2 -> track23
    MAIN,       // 3  track23  -> station3
    LOOP_LINK,  // 4  station3 -> curveB1
    TURNAROUND, // 5  curveB1  -> curveB2
    LOOP_LINK,  // 6  curveB2  -> station3
    MAIN,       // 7  station3 -> track23
    MAIN,       // 8  track23  -> station2
    MAIN,       // 9  station2 -> track12
    MAIN,       // 10 track12  -> station1
    LOOP_LINK,  // 11 station1 -> curveA1
    TURNAROUND, // 12 curveA1  -> curveA2
    LOOP_LINK,  // 13 curveA2  -> station1
];

/// Speed forced on crawl edges (curve tags "reduce the train speed") and in
/// APPROACH state.
pub const CRAWL_SPEED: f64 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RingPos {
    pub edge: usize,
    pub frac: f64,
}

pub fn nodes_for_tag(tag: &str) -> Vec<usize> {
    (0..NODES).filter(|&i| READINGS[i] == tag).collect()
}

/// The node the train crosses next when moving from `edge` in `dir`.
pub fn next_node(edge: usize, dir: Direction) -> usize {
    match dir {
        Direction::Forward => (edge + 1) % NODES,
        Direction::Reverse => edge,
    }
}

/// The edge the train is on after crossing node `node` moving in `dir`.
pub fn edge_after_crossing(node: usize, dir: Direction) -> usize {
    match dir {
        Direction::Forward => node,
        Direction::Reverse => (node + NODES - 1) % NODES,
    }
}

/// Given two consecutive readings with an unchanged motor direction, the
/// train's edge is uniquely determined: all (prev, cur) pairs are distinct
/// within each direction (asserted by test).
pub fn localize(prev: &str, cur: &str, dir: Direction) -> Option<usize> {
    (0..NODES).find_map(|j| {
        let prev_node = match dir {
            Direction::Forward => (j + NODES - 1) % NODES,
            Direction::Reverse => (j + 1) % NODES,
        };
        (READINGS[prev_node] == prev && READINGS[j] == cur).then(|| edge_after_crossing(j, dir))
    })
}

/// Station routing for move_to commands: literal expected_tags/stop_on_tag in
/// the command are ignored (the server defaults still name tag_02..tag_07);
/// only the target matters.
pub fn station_tag(target: Target) -> Option<&'static str> {
    match target {
        Target::Bar => Some("station1"),
        Target::Schachbrett => Some("station2"),
        Target::Raucherecke => Some("station3"),
        Target::ForgotName => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    #[allow(clippy::needless_range_loop)]
    fn reading_pairs_are_unique_per_direction() {
        for dir in [Direction::Forward, Direction::Reverse] {
            let mut seen = HashSet::new();
            for j in 0..NODES {
                let prev_node = match dir {
                    Direction::Forward => (j + NODES - 1) % NODES,
                    Direction::Reverse => (j + 1) % NODES,
                };
                let pair = (READINGS[prev_node], READINGS[j]);
                assert!(seen.insert(pair), "duplicate pair {pair:?} for {dir:?}");
            }
        }
    }

    #[test]
    fn localize_finds_unique_edge_in_both_directions() {
        // eastbound: passed track12 after station1 -> on edge 1
        assert_eq!(localize("station1", "track12", Direction::Forward), Some(1));
        // westbound pass of the same tags: station2 then track12 -> edge 10
        assert_eq!(
            localize("station2", "track12", Direction::Forward),
            Some(10)
        );
        // reverse motor: reading station1 after track12 while reversing
        // means we crossed node 0 downwards -> now on edge 13
        assert_eq!(
            localize("track12", "station1", Direction::Reverse),
            Some(13)
        );
        // nonsense pair
        assert_eq!(localize("station1", "station3", Direction::Forward), None);
    }

    #[test]
    fn main_tags_have_two_nodes_curve_tags_one() {
        for tag in ["station1", "track12", "station2", "track23", "station3"] {
            assert_eq!(nodes_for_tag(tag).len(), 2, "{tag}");
        }
        for tag in ["curveA1", "curveA2", "curveB1", "curveB2"] {
            assert_eq!(nodes_for_tag(tag).len(), 1, "{tag}");
        }
    }

    #[test]
    fn worst_tag_gap_stays_below_default_rfid_timeout() {
        let worst_ms = EDGES
            .iter()
            .map(|e| {
                let speed = if e.crawl { CRAWL_SPEED } else { 1.0 };
                (e.nominal_ms as f64 / speed) as u64
            })
            .max()
            .unwrap();
        let default_timeout =
            shotexpress_protocol::default_move_parameters().max_run_ms_without_tag;
        assert!(
            worst_ms < default_timeout,
            "worst gap {worst_ms} ms would trip the {default_timeout} ms RFID timeout"
        );
    }

    #[test]
    fn ring_walk_covers_all_nodes() {
        // forward walk from any edge passes every reading exactly once per lap
        let mut edge = 3;
        let mut seen = Vec::new();
        for _ in 0..NODES {
            let node = next_node(edge, Direction::Forward);
            seen.push(node);
            edge = edge_after_crossing(node, Direction::Forward);
        }
        let mut sorted = seen.clone();
        sorted.sort();
        assert_eq!(sorted, (0..NODES).collect::<Vec<_>>());
    }
}
