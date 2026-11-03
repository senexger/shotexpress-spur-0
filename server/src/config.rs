use std::env;
use std::fmt::Write as _;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    /// Port of the built-in MQTT broker (env MQTT_PORT, default 1883).
    pub mqtt_port: u16,
    pub mqtt_username: Option<String>,
    pub mqtt_password: Option<String>,
    pub mqtt_client_id: String,
    /// Where speed/accel are kept across restarts
    /// (env SETTINGS_PATH, default train-settings.json).
    pub settings_path: PathBuf,
    pub http_port: u16,
    /// Extra port for the order page, default 80 (env PUBLIC_PORT, empty = off).
    pub public_port: Option<u16>,
    /// mDNS host name to advertise, default "shotexpress" → shotexpress.local
    /// (env MDNS_HOSTNAME, empty = off).
    pub mdns_hostname: Option<String>,
    /// Path to track.toml (env TRACK_CONFIG, default repo-root track.toml).
    pub track_path: PathBuf,
    /// Directory for the fuel sensor log (env FUEL_LOG_DIR, default logs/).
    pub fuel_log_dir: PathBuf,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let mqtt_port = match env::var("MQTT_PORT") {
            Ok(value) => value
                .parse::<u16>()
                .map_err(|_| format!("Invalid MQTT_PORT value: {value}"))?,
            Err(_) => 1883,
        };

        let http_port = match env::var("PORT") {
            Ok(value) => value
                .parse::<u16>()
                .map_err(|_| format!("Invalid PORT value: {value}"))?,
            Err(_) => 3000,
        };

        let public_port = match env::var("PUBLIC_PORT") {
            Ok(value) if value.trim().is_empty() => None,
            Ok(value) => Some(
                value
                    .trim()
                    .parse::<u16>()
                    .map_err(|_| format!("Invalid PUBLIC_PORT value: {value}"))?,
            ),
            Err(_) => Some(80),
        };
        let mdns_hostname = match env::var("MDNS_HOSTNAME") {
            Ok(value) => Some(value.trim().trim_end_matches(".local").to_string())
                .filter(|v| !v.is_empty()),
            Err(_) => Some("shotexpress".to_string()),
        };

        Ok(Config {
            mqtt_port,
            mqtt_username: env::var("MQTT_USERNAME").ok().filter(|v| !v.is_empty()),
            mqtt_password: env::var("MQTT_PASSWORD").ok().filter(|v| !v.is_empty()),
            mqtt_client_id: random_client_id(),
            settings_path: env::var("SETTINGS_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("train-settings.json")),
            http_port,
            public_port,
            mdns_hostname,
            track_path: env::var("TRACK_CONFIG")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("track.toml")),
            fuel_log_dir: env::var("FUEL_LOG_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("logs")),
        })
    }
}

fn random_client_id() -> String {
    let mut id = String::from("shotexpress-server-");
    for byte in &uuid::Uuid::new_v4().as_bytes()[..4] {
        let _ = write!(id, "{byte:02x}");
    }
    id
}
