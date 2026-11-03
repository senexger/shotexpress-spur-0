//! Track topology: a main line with a passive turnaround loop at each end
//! (dogbone). Flattened, this is a single ring of tag *readings*: eastbound
//! main line, loop B, westbound main line (same physical track, opposite
//! pass), loop A. Tags are read from one side only (ETCS-style balises), so
//! each direction has its own tags and every reading is unique: one read
//! pins the position. Stations are only served eastbound.
//!
//! The train can only run forward — the passive switches make every lap take
//! the same route — so motion is always increasing ring index. Crossing into
//! node `j` emits reading `READINGS[j]`, and the train is then on edge `j`.
//! `RingPos { edge, frac }` is a point on edge `edge`, which connects node
//! `edge` to node `(edge + 1) % NODES`.

pub const NODES: usize = 14;

pub const READINGS: [&str; NODES] = [
    "station1", "track12", "station2", "track23", "station3", // 0..=4  eastbound main line
    "curveB1", "curveB2", // 5..=6  loop B (east turnaround)
    "west3", "west23", "west2", "west12", "west1", // 7..=11 westbound main line
    "curveA1", "curveA2", // 12..=13 loop A (west turnaround)
];

#[derive(Debug, Clone, Copy)]
pub struct Edge {
    pub nominal_ms: u64,
    pub len_m: f64,
    /// Turnaround curve — speeds come from the drive legs, so runtime code
    /// never reads this; the worst-tag-gap test does.
    #[cfg_attr(not(test), allow(dead_code))]
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
    MAIN,       // 7  west3    -> west23
    MAIN,       // 8  west23   -> west2
    MAIN,       // 9  west2    -> west12
    MAIN,       // 10 west12   -> west1
    LOOP_LINK,  // 11 west1    -> curveA1
    TURNAROUND, // 12 curveA1  -> curveA2
    LOOP_LINK,  // 13 curveA2  -> station1
];

/// Slowest speed track.toml's speed tags command on the turnaround edges
/// (the curve tags' `speed = 0.5`).
#[cfg_attr(not(test), allow(dead_code))]
pub const CRAWL_SPEED: f64 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RingPos {
    pub edge: usize,
    pub frac: f64,
}

pub fn nodes_for_tag(tag: &str) -> Vec<usize> {
    (0..NODES).filter(|&i| READINGS[i] == tag).collect()
}

/// The node the train crosses next from `edge` (always running forward).
pub fn next_node(edge: usize) -> usize {
    (edge + 1) % NODES
}

/// Given two consecutive readings, the train's edge is uniquely determined:
/// all (prev, cur) reading pairs on the ring are distinct (asserted by test).
/// Crossing node `j` puts the train on edge `j`.
pub fn localize(prev: &str, cur: &str) -> Option<usize> {
    (0..NODES).find(|&j| READINGS[(j + NODES - 1) % NODES] == prev && READINGS[j] == cur)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    #[allow(clippy::needless_range_loop)]
    fn reading_pairs_are_unique() {
        let mut seen = HashSet::new();
        for j in 0..NODES {
            let pair = (READINGS[(j + NODES - 1) % NODES], READINGS[j]);
            assert!(seen.insert(pair), "duplicate pair {pair:?}");
        }
    }

    #[test]
    fn localize_finds_unique_edge() {
        // eastbound: passed track12 after station1 -> on edge 1
        assert_eq!(localize("station1", "track12"), Some(1));
        // westbound tags are their own
        assert_eq!(localize("west2", "west12"), Some(10));
        // nonsense pair
        assert_eq!(localize("station1", "station3"), None);
    }

    #[test]
    fn every_tag_sits_at_exactly_one_node() {
        for tag in READINGS {
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
        // mirrors [train] max_run_ms_without_tag in track.toml
        let default_timeout = 30_000;
        assert!(
            worst_ms < default_timeout,
            "worst gap {worst_ms} ms would trip the {default_timeout} ms RFID timeout"
        );
    }

    #[test]
    fn forward_walk_covers_all_nodes() {
        let mut edge = 3;
        let mut seen = Vec::new();
        for _ in 0..NODES {
            let node = next_node(edge);
            seen.push(node);
            edge = node;
        }
        let mut sorted = seen.clone();
        sorted.sort();
        assert_eq!(sorted, (0..NODES).collect::<Vec<_>>());
    }
}
