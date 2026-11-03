use std::env;
use std::fmt::Write as _;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub mqtt_host: String,
    pub mqtt_port: u16,
    pub mqtt_username: Option<String>,
    pub mqtt_password: Option<String>,
    pub mqtt_client_id: String,
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
        let url = env::var("MQTT_URL")
            .map_err(|_| "Missing required environment variable MQTT_URL".to_string())?;
        let (mqtt_host, mqtt_port) = parse_mqtt_url(&url)?;

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
            mqtt_host,
            mqtt_port,
            mqtt_username: env::var("MQTT_USERNAME").ok().filter(|v| !v.is_empty()),
            mqtt_password: env::var("MQTT_PASSWORD").ok().filter(|v| !v.is_empty()),
            mqtt_client_id: random_client_id(),
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

fn parse_mqtt_url(url: &str) -> Result<(String, u16), String> {
    if url.starts_with("ws://") || url.starts_with("wss://") {
        return Err(format!(
            "MQTT_URL {url}: websocket URLs are not supported, use mqtt://host:port"
        ));
    }
    let rest = url
        .strip_prefix("mqtt://")
        .or_else(|| url.strip_prefix("tcp://"))
        .unwrap_or(url);
    let rest = rest.trim_end_matches('/');
    if rest.is_empty() {
        return Err(format!("MQTT_URL {url}: missing host"));
    }
    match rest.rsplit_once(':') {
        Some((host, port)) => {
            let port = port
                .parse::<u16>()
                .map_err(|_| format!("MQTT_URL {url}: invalid port {port}"))?;
            Ok((host.to_string(), port))
        }
        None => Ok((rest.to_string(), 1883)),
    }
}

fn random_client_id() -> String {
    let mut id = String::from("shotexpress-server-");
    for byte in &uuid::Uuid::new_v4().as_bytes()[..4] {
        let _ = write!(id, "{byte:02x}");
    }
    id
}

#[cfg(test)]
mod tests {
    use super::parse_mqtt_url;

    #[test]
    fn parses_urls() {
        assert_eq!(
            parse_mqtt_url("mqtt://localhost:1883").unwrap(),
            ("localhost".into(), 1883)
        );
        assert_eq!(
            parse_mqtt_url("tcp://192.168.16.127:1884").unwrap(),
            ("192.168.16.127".into(), 1884)
        );
        assert_eq!(
            parse_mqtt_url("mqtt://broker.local").unwrap(),
            ("broker.local".into(), 1883)
        );
        assert_eq!(
            parse_mqtt_url("localhost:1883").unwrap(),
            ("localhost".into(), 1883)
        );
        assert!(parse_mqtt_url("ws://localhost:9001").is_err());
    }
}
