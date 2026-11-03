//! Built-in MQTT broker (rumqttd), so the server is the only thing to start:
//! the train, the fuel station and fake-train connect to it on port 1883.
//!
//! Retained messages live in memory only: the server re-publishes the tag
//! table on every connect and restores speed/accel from `settings.rs`.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};

use rumqttd::{Broker, ConnectionSettings, RouterConfig, ServerSettings};

use crate::config::Config;

/// Start the broker on its own threads.
pub fn start(cfg: &Config) -> Result<(), String> {
    let listen = SocketAddr::from((Ipv4Addr::UNSPECIFIED, cfg.mqtt_port));

    // rumqttd only logs a failed bind from its own thread; check up front so
    // the message says what to do
    drop(TcpListener::bind(listen).map_err(|err| {
        format!(
            "MQTT broker cannot listen on {listen}: {err} \
             (another broker running? e.g. `docker compose down` in mqtt-broker/)"
        )
    })?);

    let auth = match (&cfg.mqtt_username, &cfg.mqtt_password) {
        (Some(user), Some(pass)) => Some(HashMap::from([(user.clone(), pass.clone())])),
        _ => None,
    };
    let anonymous = auth.is_none();
    let server = ServerSettings {
        name: "v4".into(),
        listen,
        tls: None,
        next_connection_delay_ms: 1,
        connections: ConnectionSettings {
            connection_timeout_ms: 60_000,
            max_payload_size: 64 * 1024,
            max_inflight_count: 100,
            auth,
            external_auth: None,
            dynamic_filters: true,
        },
    };
    let config = rumqttd::Config {
        router: RouterConfig {
            max_connections: 100,
            max_outgoing_packet_count: 200,
            // per-topic message log; a handful of small messages per second
            max_segment_size: 1024 * 1024,
            max_segment_count: 2,
            ..Default::default()
        },
        v4: Some(HashMap::from([("1".to_string(), server)])),
        ..Default::default()
    };

    std::thread::Builder::new()
        .name("mqtt-broker".into())
        .spawn(move || {
            let mut broker = Broker::new(config);
            if let Err(err) = broker.start() {
                tracing::error!(%err, "MQTT broker stopped");
            }
        })
        .map_err(|err| format!("cannot start MQTT broker thread: {err}"))?;
    tracing::info!(
        "MQTT broker listening on {listen}{}",
        if anonymous { " (no login)" } else { "" }
    );
    Ok(())
}
