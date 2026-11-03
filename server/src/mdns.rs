//! Advertise the server on the local network as `<name>.local` (mDNS A
//! records) plus an `_http._tcp` service, so guests can open
//! http://shotexpress.local on their phones.
//!
//! An `_mqtt._tcp` service is announced too, so the train finds the broker
//! whatever IP the laptop got.
//!
//! Only physical interfaces (WiFi, ethernet) are announced: Docker bridges
//! and veth pairs would hand phones an address they cannot reach.
//! `MDNS_INTERFACES=wlan0,eth0` overrides the choice.

use mdns_sd::{IfKind, ServiceDaemon, ServiceInfo};

/// Keep the returned daemon alive for as long as the name should resolve.
pub fn advertise(name: &str, port: u16, mqtt_port: u16) -> Option<ServiceDaemon> {
    let host = format!("{name}.local.");
    let interfaces = lan_interfaces();
    let result = ServiceDaemon::new().and_then(|daemon| {
        if !interfaces.is_empty() {
            daemon.disable_interface(IfKind::All)?;
            for interface in &interfaces {
                daemon.enable_interface(IfKind::Name(interface.clone()))?;
            }
        }
        // IPv4 is enough on a party LAN; link-local IPv6 answers only confuse
        daemon.disable_interface(IfKind::IPv6)?;
        let info = ServiceInfo::new("_http._tcp.local.", "ShotExpress", &host, "", port, None)?
            .enable_addr_auto();
        daemon.register(info)?;
        let info =
            ServiceInfo::new("_mqtt._tcp.local.", "ShotExpress", &host, "", mqtt_port, None)?
                .enable_addr_auto();
        daemon.register(info)?;
        Ok(daemon)
    });
    match result {
        Ok(daemon) => {
            let url = match port {
                80 => format!("http://{name}.local"),
                _ => format!("http://{name}.local:{port}"),
            };
            let on = if interfaces.is_empty() {
                "all interfaces".to_string()
            } else {
                interfaces.join(", ")
            };
            tracing::info!("order page advertised via mDNS at {url} (on {on})");
            tracing::info!("MQTT broker advertised via mDNS as _mqtt._tcp on port {mqtt_port}");
            Some(daemon)
        }
        Err(err) => {
            tracing::warn!(%err, "mDNS advertising failed; reach the server by IP instead");
            None
        }
    }
}

/// `MDNS_INTERFACES` if set, else every interface backed by real hardware
/// (Linux: `/sys/class/net/<if>/device` exists). Empty = no filter.
fn lan_interfaces() -> Vec<String> {
    if let Ok(list) = std::env::var("MDNS_INTERFACES") {
        return list
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }
    let Ok(entries) = std::fs::read_dir("/sys/class/net") else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().join("device").exists())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}
