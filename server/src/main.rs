mod assets;
mod config;
mod routes;
mod service;
mod track;
mod types;

use std::net::{Ipv4Addr, SocketAddr};
use std::process::ExitCode;

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
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
        nodes = track.nodes.len(),
        stations = track.stations.len(),
        "track config loaded"
    );

    let (service, eventloop) = service::TrainService::new(&config);
    tokio::spawn(service::run_event_loop(service.clone(), eventloop));
    tokio::spawn(service::run_liveness_sweep(service.clone()));

    let app = routes::router(std::sync::Arc::new(routes::AppState {
        service,
        track: std::sync::RwLock::new(std::sync::Arc::new(track)),
        track_path: config.track_path.clone(),
    }));
    let addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, config.http_port));
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("failed to bind {addr}: {err}");
            return ExitCode::FAILURE;
        }
    };

    tracing::info!(
        "listening on http://localhost:{} (MQTT broker {}:{})",
        config.http_port,
        config.mqtt_host,
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
