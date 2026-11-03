//! Last speed/accel settings, kept on disk because the built-in broker
//! forgets retained messages on restart. Without this the train would fall
//! back to its DEFAULT_SPEED_HZ after every server restart.
//!
//! File: `$SETTINGS_PATH` (default `train-settings.json`), e.g.
//! `{"speed_hz":1200,"accel":800}`. Only used with the built-in broker;
//! Mosquitto keeps retained messages itself.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrainSettings {
    pub speed_hz: Option<u32>,
    pub accel: Option<u32>,
}

/// Missing or broken file = no settings (logged, not fatal).
pub fn load(path: &Path) -> TrainSettings {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|err| {
            tracing::warn!(%err, path = %path.display(), "ignoring unreadable settings file");
            TrainSettings::default()
        }),
        Err(_) => TrainSettings::default(),
    }
}

/// Write via a temp file + rename, so a crash never leaves half a file.
pub fn save(path: &Path, settings: TrainSettings) {
    let tmp = path.with_extension("json.tmp");
    let result = serde_json::to_vec(&settings)
        .map_err(std::io::Error::other)
        .and_then(|bytes| fs::write(&tmp, bytes))
        .and_then(|()| fs::rename(&tmp, path));
    if let Err(err) = result {
        tracing::warn!(%err, path = %path.display(), "cannot save train settings");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dir = std::env::temp_dir().join(format!("shotexpress-settings-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("train-settings.json");
        assert_eq!(load(&path), TrainSettings::default());
        let settings = TrainSettings { speed_hz: Some(1200), accel: Some(800) };
        save(&path, settings);
        assert_eq!(load(&path), settings);
        fs::remove_dir_all(dir).unwrap();
    }
}
