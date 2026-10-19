//! The train's position belief, computed only from tag readings — never from
//! the simulation's ground truth. The train always runs forward, so the
//! reading order is fixed: spawn position is unknown; one reading narrows it
//! to the tag's candidate edges (curve tags are unique, so they localize
//! immediately); a second consecutive reading pins it down.

use crate::track::{self, NODES, READINGS};

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
    last_reading: Option<&'static str>,
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

    pub fn on_reading(&mut self, tag: &'static str) -> LocalizeOutcome {
        let outcome = match &self.belief {
            Belief::Localized { edge } => {
                let predicted_node = track::next_node(*edge);
                if READINGS[predicted_node] == tag {
                    self.belief = Belief::Localized {
                        edge: predicted_node,
                    };
                    LocalizeOutcome::Tracking(predicted_node)
                } else {
                    let expected = READINGS[predicted_node];
                    self.set_candidates(tag);
                    LocalizeOutcome::Mismatch { expected, got: tag }
                }
            }
            _ => {
                let paired = self
                    .last_reading
                    .and_then(|prev| track::localize(prev, tag));
                match paired {
                    Some(edge) => {
                        self.belief = Belief::Localized { edge };
                        LocalizeOutcome::JustLocalized(edge)
                    }
                    None => self.set_candidates(tag),
                }
            }
        };
        self.last_reading = Some(tag);
        outcome
    }

    /// A tag read while the train is driven backward (manual mode only).
    /// The pair table assumes forward reading order, so no pair inference —
    /// but an existing localization is tracked, and unique tags still pin
    /// the position.
    pub fn on_reading_backward(&mut self, tag: &'static str) -> LocalizeOutcome {
        self.last_reading = None; // forward pairs must not span backward reads
        match &self.belief {
            Belief::Localized { edge } => {
                // moving backward on edge e, the next node crossed is node e
                if READINGS[*edge] == tag {
                    let new_edge = (*edge + NODES - 1) % NODES;
                    self.belief = Belief::Localized { edge: new_edge };
                    LocalizeOutcome::Tracking(new_edge)
                } else {
                    let expected = READINGS[*edge];
                    self.set_candidates_backward(tag);
                    LocalizeOutcome::Mismatch { expected, got: tag }
                }
            }
            _ => self.set_candidates_backward(tag),
        }
    }

    /// Predicted next reading, if localized.
    pub fn predicted_next(&self) -> Option<&'static str> {
        match &self.belief {
            Belief::Localized { edge } => Some(READINGS[track::next_node(*edge)]),
            _ => None,
        }
    }

    fn set_candidates(&mut self, tag: &'static str) -> LocalizeOutcome {
        // crossing node j forward puts the train on edge j
        self.apply_candidates(tag, track::nodes_for_tag(tag))
    }

    fn set_candidates_backward(&mut self, tag: &'static str) -> LocalizeOutcome {
        // crossing node j backward puts the train on edge j - 1
        let edges = track::nodes_for_tag(tag)
            .into_iter()
            .map(|node| (node + NODES - 1) % NODES)
            .collect();
        self.apply_candidates(tag, edges)
    }

    fn apply_candidates(&mut self, tag: &'static str, edges: Vec<usize>) -> LocalizeOutcome {
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

    /// Canonical reading for a node index, as a `&'static str` from READINGS.
    fn tag(node: usize) -> &'static str {
        READINGS[node % NODES]
    }

    #[test]
    fn single_main_tag_gives_two_candidates() {
        let mut loc = Localizer::new();
        let outcome = loc.on_reading(tag(2)); // station2
        assert_eq!(outcome, LocalizeOutcome::Candidates(2));
        match &loc.belief {
            Belief::Candidates { edges, .. } => assert_eq!(edges, &vec![2, 9]),
            other => panic!("expected candidates, got {other:?}"),
        }
    }

    #[test]
    fn curve_tag_localizes_immediately() {
        let mut loc = Localizer::new();
        let outcome = loc.on_reading("curveB1");
        assert_eq!(outcome, LocalizeOutcome::JustLocalized(5));
    }

    #[test]
    fn two_readings_localize() {
        let mut loc = Localizer::new();
        loc.on_reading("station1");
        let outcome = loc.on_reading("track12");
        assert_eq!(outcome, LocalizeOutcome::JustLocalized(1));

        // the westbound pass of the same tags resolves differently
        let mut loc = Localizer::new();
        loc.on_reading("station2");
        let outcome = loc.on_reading("track12");
        assert_eq!(outcome, LocalizeOutcome::JustLocalized(10));
    }

    #[test]
    fn mismatch_falls_back_to_candidates() {
        let mut loc = Localizer::new();
        loc.on_reading("station1");
        loc.on_reading("track12"); // localized on edge 1
        // teleported: reads station3 instead of predicted station2
        let outcome = loc.on_reading("station3");
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
    fn backward_reads_track_but_do_not_pair() {
        // localized on edge 1 (between track12 and station2)
        let mut loc = Localizer::new();
        loc.on_reading("station1");
        loc.on_reading("track12");
        // backing up crosses track12 (node 1) again -> edge 0
        assert_eq!(
            loc.on_reading_backward("track12"),
            LocalizeOutcome::Tracking(0)
        );
        // and station1 (node 0) -> edge 13
        assert_eq!(
            loc.on_reading_backward("station1"),
            LocalizeOutcome::Tracking(13)
        );

        // unlocalized: a backward main-line read gives candidates, and the
        // next forward read must NOT pair with it
        let mut loc = Localizer::new();
        assert_eq!(
            loc.on_reading_backward("station2"),
            LocalizeOutcome::Candidates(2)
        );
        assert_eq!(loc.on_reading("track23"), LocalizeOutcome::Candidates(2));
    }

    #[test]
    fn full_lap_tracks_every_node() {
        let mut loc = Localizer::new();
        loc.on_reading(tag(0));
        loc.on_reading(tag(1));
        let mut edge = 1;
        for node in 2..(2 * NODES) {
            let outcome = loc.on_reading(tag(node));
            edge = (edge + 1) % NODES;
            assert_eq!(outcome, LocalizeOutcome::Tracking(edge));
        }
    }
}
