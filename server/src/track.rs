//! The track map (`track.toml`): a flat list of tags, like ETCS balises.
//! Every tag is read from one side only, so its uid alone says where the
//! train is and which way it is heading. A tag either sets the speed from
//! there on, or is a station (full stop when it is the ride's target).
//! The server publishes this as the train's tag table; a ride only names
//! the target station.

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};
use shotexpress_protocol::{TagKind, TagRule, TrackTable};

#[derive(Debug, Clone, Deserialize)]
pub struct TrackConfig {
    pub train: TrainParams,
    pub refill: RefillParams,
    #[serde(rename = "station")]
    pub stations: Vec<Station>,
    #[serde(rename = "speed", default)]
    pub speeds: Vec<SpeedTag>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TrainParams {
    pub max_speed: f64,
    pub start_speed: f64,
    pub accel: f64,
    /// RFID watchdog: the train stops itself after this long without a
    /// tag. 0 = off.
    pub max_run_ms_without_tag: u64,
    pub default_ttl_ms: u64,
}

impl TrainParams {
    /// The value a ride carries. The firmware compares against a 32-bit
    /// millis() difference, so u32::MAX never fires (and a bigger number
    /// would make ArduinoJson fall back to its 30 s default).
    pub fn watchdog_ms(&self) -> u64 {
        match self.max_run_ms_without_tag {
            0 => u32::MAX as u64,
            ms => ms.min(u32::MAX as u64),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RefillParams {
    pub station: String,
    pub steps: u32,
    pub speed: f64,
    /// One shot, in ml (what the fuel station pours per glass).
    #[serde(default = "default_shot_ml")]
    pub shot_ml: u32,
    /// Most shots one order may ask for (glasses on the wagon).
    #[serde(default = "default_max_shots")]
    pub max_shots: u32,
    /// The firmware reports no `completed` for `steps` yet, so an order
    /// waits this long for a nudge to finish before the next pour.
    #[serde(default = "default_nudge_wait_ms")]
    pub nudge_wait_ms: u64,
    /// false: shots are filled by hand, an order only drives to its station.
    #[serde(default = "default_auto_fill")]
    pub auto_fill: bool,
}

fn default_auto_fill() -> bool {
    true
}

fn default_shot_ml() -> u32 {
    20
}

fn default_max_shots() -> u32 {
    6
}

fn default_nudge_wait_ms() -> u64 {
    3000
}

/// What happens at a station: shots are taken off (`unload`) or the
/// fuel station fills new ones (`refill`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StationRole {
    Unload,
    Refill,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Station {
    pub name: String,
    pub tag: String,
    pub role: StationRole,
    /// After arriving, wait `wait_s` seconds, then drive on to `then`.
    #[serde(default)]
    pub then: Option<String>,
    #[serde(default)]
    pub wait_s: u64,
}

/// "From this tag on, at most `speed`."
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SpeedTag {
    pub tag: String,
    pub speed: f64,
    /// Free text for humans ("curve B", "switch west").
    #[serde(default)]
    pub note: Option<String>,
}

fn check_speed(what: &str, speed: f64) -> Result<(), String> {
    if speed > 0.0 && speed <= 1.0 {
        Ok(())
    } else {
        Err(format!("{what}: speed {speed} must be in (0, 1]"))
    }
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
        check_speed("train.max_speed", self.train.max_speed)?;
        check_speed("train.start_speed", self.train.start_speed)?;
        if self.train.accel <= 0.0 {
            return Err("train.accel must be positive".into());
        }

        // one meaning per tag: a uid is either a station or a speed tag
        let mut uids = HashSet::new();
        let mut names = HashSet::new();
        for s in &self.stations {
            if !names.insert(s.name.as_str()) {
                return Err(format!("station {} is defined twice", s.name));
            }
            // an empty tag is a placeholder: not stuck on the track yet
            if !s.tag.is_empty() && !uids.insert(s.tag.as_str()) {
                return Err(format!("tag {} is used twice", s.tag));
            }
        }
        for t in &self.speeds {
            check_speed(&format!("speed tag {}", t.tag), t.speed)?;
            if !t.tag.is_empty() && !uids.insert(t.tag.as_str()) {
                return Err(format!("tag {} is used twice", t.tag));
            }
        }

        for s in &self.stations {
            if let Some(next) = &s.then {
                if self.station(next).is_none() {
                    return Err(format!("station {}: then = {next} is not a station", s.name));
                }
                if next == &s.name {
                    return Err(format!("station {}: then must name another station", s.name));
                }
            }
        }

        match self.station(&self.refill.station) {
            None => Err(format!(
                "refill.station {} is not a station",
                self.refill.station
            )),
            Some(s) if s.role != StationRole::Refill => Err(format!(
                "refill.station {} has role {:?}, expected refill",
                s.name, s.role
            )),
            Some(_) if self.refill.shot_ml == 0 || self.refill.max_shots == 0 => {
                Err("refill.shot_ml and refill.max_shots must be positive".into())
            }
            Some(_) => Ok(()),
        }
    }

    pub fn station(&self, name: &str) -> Option<&Station> {
        self.stations.iter().find(|s| s.name == name)
    }

    /// The tag table the train caches (published retained). Tags not
    /// assigned yet (empty uid) are left out.
    pub fn table(&self) -> TrackTable {
        let tags: Vec<TagRule> = self
            .stations
            .iter()
            .filter(|s| !s.tag.is_empty())
            .map(|s| TagRule {
                uid: s.tag.clone(),
                kind: TagKind::Station {
                    station: s.name.clone(),
                },
            })
            .chain(
                self.speeds
                    .iter()
                    .filter(|t| !t.tag.is_empty())
                    .map(|t| TagRule {
                        uid: t.tag.clone(),
                        kind: TagKind::Speed { speed: t.speed },
                    }),
            )
            .collect();
        let json = serde_json::to_vec(&tags).expect("tag table serializes");
        TrackTable {
            version: fnv1a(&json),
            tags,
        }
    }
}

/// Stable 32-bit hash (FNV-1a): same tags, same version, across restarts.
fn fnv1a(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0x811c_9dc5u32, |hash, b| {
        (hash ^ *b as u32).wrapping_mul(0x0100_0193)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const FIXTURE: &str = r#"
[train]
max_speed = 1.0
start_speed = 0.3
accel = 0.5
max_run_ms_without_tag = 30000
default_ttl_ms = 60000

[refill]
station = "bar"
steps = 1000
speed = 0.2

[[station]]
name = "bar"
tag = "AA000001"
role = "refill"

[[station]]
name = "raucherecke"
tag = "AA000002"
role = "unload"

[[speed]]
tag = "BB000001"
speed = 0.4
"#;

    fn fixture() -> TrackConfig {
        let track: TrackConfig = toml::from_str(FIXTURE).unwrap();
        track.validate().unwrap();
        track
    }

    #[test]
    fn repo_track_toml_loads_and_validates() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../track.toml");
        let track = TrackConfig::load(&path).expect("repo track.toml must be valid");
        assert!(track.station(&track.refill.station).is_some());
    }

    #[test]
    fn table_lists_stations_and_speed_tags() {
        let track = fixture();
        let table = track.table();
        assert_eq!(table.tags.len(), track.stations.len() + track.speeds.len());
        assert_eq!(table.station_uid("bar"), Some("AA000001"));
        assert_eq!(
            table.rule("BB000001"),
            Some(&TagKind::Speed { speed: 0.4 })
        );
    }

    #[test]
    fn empty_tags_are_placeholders() {
        let mut track = fixture();
        track.stations[1].tag.clear();
        track.speeds[0].tag.clear();
        track.speeds.push(track.speeds[0].clone());
        assert!(track.validate().is_ok(), "empty tags never clash");
        let table = track.table();
        assert_eq!(table.tags.len(), 1, "only the bar has a uid");
        assert!(table.rule("").is_none());
    }

    #[test]
    fn table_version_follows_content() {
        let track = fixture();
        assert_eq!(track.table().version, fixture().table().version);
        let mut changed = fixture();
        changed.speeds[0].speed /= 2.0;
        assert_ne!(track.table().version, changed.table().version);
    }

    #[test]
    fn validation_rejects_broken_configs() {
        let mut track = fixture();
        track.speeds[0].speed = 0.0;
        assert!(track.validate().is_err(), "speed must be positive");

        let mut track = fixture();
        track.speeds[0].tag = track.stations[0].tag.clone();
        assert!(track.validate().is_err(), "a tag has one meaning");

        let mut track = fixture();
        let dup = track.speeds[0].clone();
        track.speeds.push(dup);
        assert!(track.validate().is_err(), "duplicate speed tag");

        let mut track = fixture();
        track.stations[0].role = StationRole::Unload;
        assert!(track.validate().is_err(), "refill.station must refill");

        let mut track = fixture();
        track.refill.station = "nowhere".into();
        assert!(track.validate().is_err(), "refill.station must exist");

        let mut track = fixture();
        track.stations[1].then = Some("nowhere".into());
        assert!(track.validate().is_err(), "then must be a station");

        let mut track = fixture();
        track.stations[1].then = Some("raucherecke".into());
        assert!(track.validate().is_err(), "then must be another station");

        let mut track = fixture();
        track.stations[1].then = Some("bar".into());
        assert!(track.validate().is_ok());

        let mut track = fixture();
        track.train.max_run_ms_without_tag = 0;
        assert_eq!(track.train.watchdog_ms(), u32::MAX as u64, "0 = off");
        track.train.max_run_ms_without_tag = 30_000;
        assert_eq!(track.train.watchdog_ms(), 30_000);

        let mut track = fixture();
        track.train.start_speed = 1.5;
        assert!(track.validate().is_err());
    }
}
