mod assets;
mod broker;
mod config;
mod fuel_log;
mod mdns;
mod orders;
mod routes;
mod service;
mod settings;
mod track;
mod trip;
mod types;

use std::net::{Ipv4Addr, SocketAddr};
use std::process::ExitCode;

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            // rumqttd logs every connection at info, and a harmless
            // "connection id N is already gone" error at every disconnect
            EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                EnvFilter::new("info,rumqttd=warn,rumqttd::router::routing=off")
            }),
        )
        .init();

    let config = match config::Config::from_env() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("configuration error: {err}");
            return ExitCode::FAILURE;
        }
    };

    let track = match track::TrackConfig::load(&config.track_path) {
        Ok(track) => track,
        Err(err) => {
            eprintln!("track config error: {err}");
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(
        path = %config.track_path.display(),
        stations = track.stations.len(),
        speed_tags = track.speeds.len(),
        "track config loaded"
    );

    if let Err(err) = broker::start(&config) {
        eprintln!("{err}");
        return ExitCode::FAILURE;
    }
    let (service, eventloop) = service::TrainService::new(&config);
    service.set_track_table(track.table());
    tokio::spawn(service::run_event_loop(service.clone(), eventloop));
    tokio::spawn(service::run_liveness_sweep(service.clone()));

    let track = std::sync::Arc::new(std::sync::RwLock::new(std::sync::Arc::new(track)));
    let orders = orders::OrderQueue::new();
    tokio::spawn(orders::run_worker(orders.clone(), service.clone(), track.clone()));

    let app = routes::router(std::sync::Arc::new(routes::AppState {
        service,
        track,
        track_path: config.track_path.clone(),
        orders,
        trip: trip::Trip::new(),
    }));
    let addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, config.http_port));
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("failed to bind {addr}: {err}");
            return ExitCode::FAILURE;
        }
    };

    // port 80 too, so http://shotexpress.local works without a port number;
    // best effort, since ports below 1024 need privileges
    let mut page_port = config.http_port;
    if let Some(public_port) = config.public_port.filter(|p| *p != config.http_port) {
        let public_addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, public_port));
        match tokio::net::TcpListener::bind(public_addr).await {
            Ok(public_listener) => {
                page_port = public_port;
                let app = app.clone();
                tokio::spawn(async move {
                    if let Err(err) = axum::serve(public_listener, app).await {
                        tracing::warn!(%err, "server on port {public_port} stopped");
                    }
                });
            }
            Err(err) => tracing::warn!(
                %err,
                "cannot listen on port {public_port}; run as root or grant it once with \
                 `sudo setcap cap_net_bind_service=+ep <binary>` (PUBLIC_PORT= disables this)"
            ),
        }
    }

    let _mdns = config
        .mdns_hostname
        .as_deref()
        .and_then(|name| mdns::advertise(name, page_port, config.mqtt_port));

    tracing::info!(
        "listening on http://localhost:{} (MQTT on port {})",
        config.http_port,
        config.mqtt_port
    );

    // No graceful drain: SSE connections stay open indefinitely (heartbeats),
    // so waiting for them would hang Ctrl+C. Abort serving right away instead.
    let result = tokio::select! {
        result = axum::serve(listener, app) => result,
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("shutting down");
            Ok(())
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("server error: {err}");
            ExitCode::FAILURE
        }
    }
}
