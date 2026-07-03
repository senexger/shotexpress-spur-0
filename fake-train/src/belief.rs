//! The train's position belief, computed only from tag readings and the
//! (always known) motor direction — never from the simulation's ground truth.
//! Spawn position is unknown; one reading narrows it to the tag's candidate
//! edges (curve tags are unique, so they localize immediately); a second
//! consecutive reading with unchanged motor direction pins it down.

use crate::track::{self, READINGS};
use shotexpress_protocol::Direction;

#[derive(Debug, Clone, PartialEq)]
pub enum Belief {
    Unknown,
    Candidates {
        tag: &'static str,
        edges: Vec<usize>,
    },
    /// The edge the train is currently on.
    Localized {
        edge: usize,
    },
}

#[derive(Debug)]
pub struct Localizer {
    pub belief: Belief,
    last_reading: Option<(&'static str, Direction)>,
}

#[derive(Debug, PartialEq)]
pub enum LocalizeOutcome {
    Candidates(usize),
    JustLocalized(usize),
    Tracking(usize),
    /// Prediction contradicted (e.g. after the position was randomized).
    Mismatch {
        expected: &'static str,
        got: &'static str,
    },
}

impl Localizer {
    pub fn new() -> Self {
        Localizer {
            belief: Belief::Unknown,
            last_reading: None,
        }
    }

    pub fn reset(&mut self) {
        self.belief = Belief::Unknown;
        self.last_reading = None;
    }

    /// A pair of readings only identifies the edge if the motor direction did
    /// not change in between; an already-localized belief stays valid.
    pub fn on_direction_change(&mut self) {
        self.last_reading = None;
    }

    pub fn on_reading(&mut self, tag: &'static str, dir: Direction) -> LocalizeOutcome {
        let outcome = match &self.belief {
            Belief::Localized { edge } => {
                let predicted_node = track::next_node(*edge, dir);
                if READINGS[predicted_node] == tag {
                    let new_edge = track::edge_after_crossing(predicted_node, dir);
                    self.belief = Belief::Localized { edge: new_edge };
                    LocalizeOutcome::Tracking(new_edge)
                } else {
                    let expected = READINGS[predicted_node];
                    self.set_candidates(tag, dir);
                    LocalizeOutcome::Mismatch { expected, got: tag }
                }
            }
            _ => {
                let paired = self
                    .last_reading
                    .filter(|(_, prev_dir)| *prev_dir == dir)
                    .and_then(|(prev, _)| track::localize(prev, tag, dir));
                match paired {
                    Some(edge) => {
                        self.belief = Belief::Localized { edge };
                        LocalizeOutcome::JustLocalized(edge)
                    }
                    None => self.set_candidates(tag, dir),
                }
            }
        };
        self.last_reading = Some((tag, dir));
        outcome
    }

    /// Predicted next reading, if localized.
    pub fn predicted_next(&self, dir: Direction) -> Option<&'static str> {
        match &self.belief {
            Belief::Localized { edge } => Some(READINGS[track::next_node(*edge, dir)]),
            _ => None,
        }
    }

    fn set_candidates(&mut self, tag: &'static str, dir: Direction) -> LocalizeOutcome {
        let edges: Vec<usize> = track::nodes_for_tag(tag)
            .into_iter()
            .map(|node| track::edge_after_crossing(node, dir))
            .collect();
        if edges.len() == 1 {
            let edge = edges[0];
            self.belief = Belief::Localized { edge };
            LocalizeOutcome::JustLocalized(edge)
        } else {
            self.belief = Belief::Candidates {
                tag,
                edges: edges.clone(),
            };
            LocalizeOutcome::Candidates(edges.len())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::NODES;

    /// Canonical reading for a node index, as a `&'static str` from READINGS.
    fn tag(node: usize) -> &'static str {
        READINGS[node % NODES]
    }

    #[test]
    fn single_main_tag_gives_two_candidates() {
        let mut loc = Localizer::new();
        let outcome = loc.on_reading(tag(2), Direction::Forward); // station2
        assert_eq!(outcome, LocalizeOutcome::Candidates(2));
        match &loc.belief {
            Belief::Candidates { edges, .. } => assert_eq!(edges, &vec![2, 9]),
            other => panic!("expected candidates, got {other:?}"),
        }
    }

    #[test]
    fn curve_tag_localizes_immediately() {
        let mut loc = Localizer::new();
        let outcome = loc.on_reading("curveB1", Direction::Forward);
        assert_eq!(outcome, LocalizeOutcome::JustLocalized(5));
    }

    #[test]
    fn two_readings_localize_in_both_directions() {
        let mut loc = Localizer::new();
        loc.on_reading("station1", Direction::Forward);
        let outcome = loc.on_reading("track12", Direction::Forward);
        assert_eq!(outcome, LocalizeOutcome::JustLocalized(1));

        let mut loc = Localizer::new();
        loc.on_reading("track12", Direction::Reverse);
        let outcome = loc.on_reading("station1", Direction::Reverse);
        assert_eq!(outcome, LocalizeOutcome::JustLocalized(13));
    }

    #[test]
    fn direction_change_drops_the_pair_but_keeps_localization() {
        let mut loc = Localizer::new();
        loc.on_reading("station1", Direction::Forward);
        loc.on_direction_change();
        // pair (station1, station2) does not exist; without the drop this
        // would mis-localize, with it we get fresh candidates
        let outcome = loc.on_reading("station2", Direction::Reverse);
        assert_eq!(outcome, LocalizeOutcome::Candidates(2));

        // once localized, reversing keeps the edge and flips the prediction
        let mut loc = Localizer::new();
        loc.on_reading("station1", Direction::Forward);
        loc.on_reading("track12", Direction::Forward); // edge 1
        loc.on_direction_change();
        assert_eq!(loc.predicted_next(Direction::Reverse), Some("track12"));
        let outcome = loc.on_reading("track12", Direction::Reverse);
        assert_eq!(outcome, LocalizeOutcome::Tracking(0));
    }

    #[test]
    fn mismatch_falls_back_to_candidates() {
        let mut loc = Localizer::new();
        loc.on_reading("station1", Direction::Forward);
        loc.on_reading("track12", Direction::Forward); // localized on edge 1
        // teleported: reads station3 instead of predicted station2
        let outcome = loc.on_reading("station3", Direction::Forward);
        assert_eq!(
            outcome,
            LocalizeOutcome::Mismatch {
                expected: "station2",
                got: "station3"
            }
        );
        assert!(matches!(loc.belief, Belief::Candidates { .. }));
    }

    #[test]
    fn full_lap_tracks_every_node() {
        let mut loc = Localizer::new();
        loc.on_reading(tag(0), Direction::Forward);
        loc.on_reading(tag(1), Direction::Forward);
        let mut edge = 1;
        for node in 2..(2 * NODES) {
            let outcome = loc.on_reading(tag(node), Direction::Forward);
            edge = (edge + 1) % NODES;
            assert_eq!(outcome, LocalizeOutcome::Tracking(edge));
        }
    }
}
