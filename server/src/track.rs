//! The track map (`track.toml`): the server-side knowledge the train
//! deliberately doesn't have. Loading validates the config against the
//! physics (RFID watchdog, braking distance) so impossible setups fail at
//! startup instead of on the rails, and `compile_route` turns "go to
//! station X" into the leg schedule the train executes.

use std::path::Path;

use serde::{Deserialize, Serialize};
use shotexpress_protocol::Leg;

#[derive(Debug, Clone, Deserialize)]
pub struct TrackConfig {
    pub train: TrainParams,
    pub refill: RefillParams,
    #[serde(rename = "station")]
    pub stations: Vec<Station>,
    #[serde(rename = "node")]
    pub nodes: Vec<Node>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TrainParams {
    pub max_speed: f64,
    pub default_speed: f64,
    pub accel: f64,
    pub max_run_ms_without_tag: u64,
    pub default_ttl_ms: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RefillParams {
    pub station: String,
    pub steps: u32,
    pub speed: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Station {
    pub name: String,
    pub tag: String,
    pub approach_speed: f64,
    #[serde(default)]
    pub pass_through_speed: Option<f64>,
}

/// A ring node: `tag` is read when crossing into it; the edge fields
/// describe the track *leaving* it (toward the next node).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Node {
    pub tag: String,
    pub edge_ms: u64,
    pub edge_len_m: f64,
    pub speed_limit: f64,
}

impl TrackConfig {
    pub fn load(path: &Path) -> Result<Self, String> {
        let raw = std::fs::read_to_string(path)
            .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
        let config: TrackConfig =
            toml::from_str(&raw).map_err(|err| format!("{} is invalid: {err}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), String> {
        let n = self.nodes.len();
        if n < 2 {
            return Err("track needs at least 2 nodes".into());
        }

        for (i, node) in self.nodes.iter().enumerate() {
            if !(0.0..=1.0).contains(&node.speed_limit) || node.speed_limit <= 0.0 {
                return Err(format!(
                    "node {i} ({}): speed_limit must be in (0, 1]",
                    node.tag
                ));
            }
            if node.edge_ms == 0 {
                return Err(format!("node {i} ({}): edge_ms must be positive", node.tag));
            }
        }
        for s in &self.stations {
            if !(0.0..=1.0).contains(&s.approach_speed) || s.approach_speed <= 0.0 {
                return Err(format!(
                    "station {}: approach_speed must be in (0, 1]",
                    s.name
                ));
            }
            if !self.nodes.iter().any(|node| node.tag == s.tag) {
                return Err(format!(
                    "station {}: tag {} is not on the ring",
                    s.name, s.tag
                ));
            }
        }
        if self.station(&self.refill.station).is_none() {
            return Err(format!(
                "refill.station {} is not a station",
                self.refill.station
            ));
        }

        // Localization needs unique consecutive reading pairs: the server
        // pins the train's ring node from two successive progress tags.
        for j in 0..n {
            let pair = (&self.nodes[(j + n - 1) % n].tag, &self.nodes[j].tag);
            for k in (j + 1)..n {
                let other = (&self.nodes[(k + n - 1) % n].tag, &self.nodes[k].tag);
                if pair == other {
                    return Err(format!(
                        "reading pair ({} -> {}) appears at nodes {j} and {k}: \
                         position would be ambiguous",
                        pair.0, pair.1
                    ));
                }
            }
        }

        // RFID watchdog: the slowest speed any compiled leg can use on an
        // edge must still reach the next tag within max_run_ms_without_tag.
        for i in 0..n {
            let entered = &self.nodes[(i + 1) % n].tag;
            let slowest = self.slowest_leg_speed(i, entered);
            let worst_ms = self.nodes[i].edge_ms as f64 / slowest;
            if worst_ms >= self.train.max_run_ms_without_tag as f64 {
                return Err(format!(
                    "edge {} -> {} takes {worst_ms:.0} ms at its slowest compiled \
                     speed {slowest}, tripping the {} ms RFID watchdog; raise the \
                     speed or the watchdog",
                    self.nodes[i].tag, entered, self.train.max_run_ms_without_tag
                ));
            }
        }

        // Braking: slowing from max_speed to a station's approach_speed must
        // fit within the edge leading into it, even at the shortest possible
        // traversal (full speed). Otherwise the crawl tag sits too close.
        for s in &self.stations {
            let ramp_ms = (self.train.max_speed - s.approach_speed) / self.train.accel * 1000.0;
            for j in 0..n {
                if self.nodes[j].tag == s.tag {
                    let edge_in_ms = self.nodes[(j + n - 1) % n].edge_ms as f64;
                    if ramp_ms > edge_in_ms {
                        return Err(format!(
                            "station {}: braking to approach_speed {} needs {ramp_ms:.0} ms \
                             but the edge into node {j} only takes {edge_in_ms:.0} ms; \
                             raise accel or approach_speed",
                            s.name, s.approach_speed
                        ));
                    }
                }
            }
        }

        Ok(())
    }

    pub fn station(&self, name: &str) -> Option<&Station> {
        self.stations.iter().find(|s| s.name == name)
    }

    fn station_by_tag(&self, tag: &str) -> Option<&Station> {
        self.stations.iter().find(|s| s.tag == tag)
    }

    /// The unique ring node crossed when `cur` was read right after `prev`
    /// (pair uniqueness is enforced by validation).
    pub fn node_for_pair(&self, prev: &str, cur: &str) -> Option<usize> {
        let n = self.nodes.len();
        (0..n).find(|&j| self.nodes[(j + n - 1) % n].tag == prev && self.nodes[j].tag == cur)
    }

    /// The lowest speed `compile_route` could ever assign to the leg over
    /// edge `i` into a node reading `entered_tag`.
    fn slowest_leg_speed(&self, i: usize, entered_tag: &str) -> f64 {
        let mut speed = self.nodes[i].speed_limit;
        if let Some(s) = self.station_by_tag(entered_tag) {
            speed = speed.min(s.approach_speed);
            if let Some(pass) = s.pass_through_speed {
                speed = speed.min(pass);
            }
        }
        speed
    }

    /// Compile "go to `station`" into a leg schedule. `from_node` is the
    /// train's last known ring node; without one the schedule covers the
    /// whole ring — wherever the train really is, its first read resyncs it
    /// (forward-only ring), at worst costing one lap.
    pub fn compile_route(
        &self,
        from_node: Option<usize>,
        station_name: &str,
        speed: Option<f64>,
    ) -> Result<Vec<Leg>, String> {
        let station = self
            .station(station_name)
            .ok_or_else(|| format!("unknown station {station_name}"))?;
        let n = self.nodes.len();
        let cruise = speed
            .unwrap_or(self.train.default_speed)
            .min(self.train.max_speed);

        let (start, len) = match from_node {
            // nearest occurrence of the stop tag wins
            Some(from) => {
                let steps = (1..=n)
                    .find(|&d| self.nodes[(from + d) % n].tag == station.tag)
                    .expect("station tag exists on the ring (validated)");
                (from, steps)
            }
            // unknown position: full ring, ending on the stop tag
            None => {
                let stop = self
                    .nodes
                    .iter()
                    .position(|node| node.tag == station.tag)
                    .expect("station tag exists on the ring (validated)");
                (stop, n)
            }
        };

        let mut legs = Vec::with_capacity(len);
        for d in 1..=len {
            let node = (start + d) % n;
            let edge = (node + n - 1) % n;
            let tag = &self.nodes[node].tag;
            let mut leg_speed = cruise.min(self.nodes[edge].speed_limit);
            if d == len {
                leg_speed = leg_speed.min(station.approach_speed);
            } else if let Some(s) = self.station_by_tag(tag)
                && let Some(pass) = s.pass_through_speed
            {
                // realism: roll gently through stations we don't stop at
                leg_speed = leg_speed.min(pass);
            }
            legs.push(Leg {
                tag: tag.clone(),
                speed: leg_speed,
            });
        }
        Ok(legs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn repo_track() -> TrackConfig {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../track.toml");
        TrackConfig::load(&path).expect("repo track.toml must be valid")
    }

    #[test]
    fn repo_track_toml_loads_and_validates() {
        let track = repo_track();
        assert_eq!(track.nodes.len(), 14);
        assert_eq!(track.stations.len(), 3);
        assert_eq!(track.refill.station, "bar");
    }

    #[test]
    fn node_for_pair_disambiguates_passes() {
        let track = repo_track();
        // eastbound: station1 then track12 -> node 1
        assert_eq!(track.node_for_pair("station1", "track12"), Some(1));
        // westbound pass of the same tags: station2 then track12 -> node 10
        assert_eq!(track.node_for_pair("station2", "track12"), Some(10));
        assert_eq!(track.node_for_pair("station1", "station3"), None);
    }

    #[test]
    fn compile_from_known_node_stops_at_nearest_pass() {
        let track = repo_track();
        // just crossed node 0 (station1 eastbound), going to raucherecke
        let legs = track.compile_route(Some(0), "raucherecke", None).unwrap();
        let tags: Vec<&str> = legs.iter().map(|l| l.tag.as_str()).collect();
        assert_eq!(tags, ["track12", "station2", "track23", "station3"]);
        // cruise on plain track, pass-through cap at station2, approach at the end
        assert_eq!(legs[0].speed, 0.6);
        assert_eq!(legs[1].speed, 0.4);
        assert_eq!(legs[3].speed, 0.35);
    }

    #[test]
    fn compile_applies_curve_speed_limits() {
        let track = repo_track();
        // from node 4 (entering loop B): the leg through the turnaround
        // edge (node 5 -> 6) is capped at the curve's speed_limit
        let legs = track.compile_route(Some(4), "raucherecke", None).unwrap();
        let tags: Vec<&str> = legs.iter().map(|l| l.tag.as_str()).collect();
        assert_eq!(tags, ["curveB1", "curveB2", "station3"]);
        assert_eq!(legs[1].speed, 0.5, "turnaround edge capped");
        assert_eq!(legs[2].speed, 0.35, "approach into the stop");
    }

    #[test]
    fn compile_without_position_covers_the_full_ring() {
        let track = repo_track();
        let legs = track.compile_route(None, "bar", None).unwrap();
        assert_eq!(legs.len(), track.nodes.len(), "full lap");
        assert_eq!(legs.last().unwrap().tag, "station1");
        // every ring tag appears, so any real position can resync
        for node in &track.nodes {
            assert!(
                legs.iter().any(|l| l.tag == node.tag),
                "{} missing",
                node.tag
            );
        }
    }

    #[test]
    fn compile_clamps_requested_speed() {
        let track = repo_track();
        let legs = track
            .compile_route(Some(0), "schachbrett", Some(9.0))
            .unwrap();
        assert!(legs.iter().all(|l| l.speed <= track.train.max_speed));
    }

    #[test]
    fn unknown_station_is_an_error() {
        let track = repo_track();
        assert!(track.compile_route(None, "forgot_name", None).is_err());
    }

    #[test]
    fn validation_rejects_broken_configs() {
        let mut track = repo_track();
        track.stations[0].approach_speed = 0.1; // 2000 ms / 0.1 = 20 s > watchdog
        assert!(track.validate().is_err());

        let mut track = repo_track();
        track.train.accel = 0.1; // braking from 1.0 to 0.35 needs 6.5 s > edge
        assert!(track.validate().is_err());

        let mut track = repo_track();
        track.nodes[7].tag = "track12".into(); // duplicates pair (curveB2? ->) — makes pairs collide
        track.nodes[8].tag = "station2".into();
        track.nodes[6].tag = "station1".into();
        assert!(track.validate().is_err());
    }
}
