use std::env;
use std::fmt::Write as _;

#[derive(Debug, Clone)]
pub struct Config {
    pub mqtt_host: String,
    pub mqtt_port: u16,
    pub mqtt_username: Option<String>,
    pub mqtt_password: Option<String>,
    pub mqtt_client_id: String,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let url = env::var("MQTT_URL")
            .map_err(|_| "Missing required environment variable MQTT_URL".to_string())?;
        let (mqtt_host, mqtt_port) = parse_mqtt_url(&url)?;

        Ok(Config {
            mqtt_host,
            mqtt_port,
            mqtt_username: env::var("MQTT_USERNAME").ok().filter(|v| !v.is_empty()),
            mqtt_password: env::var("MQTT_PASSWORD").ok().filter(|v| !v.is_empty()),
            mqtt_client_id: random_client_id(),
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
    let mut id = String::from("fake-train-");
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
            parse_mqtt_url("mqtt://broker.local").unwrap(),
            ("broker.local".into(), 1883)
        );
        assert!(parse_mqtt_url("ws://localhost:9001").is_err());
    }
}
