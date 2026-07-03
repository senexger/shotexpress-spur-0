mod assets;
mod config;
mod routes;
mod service;
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

    let (service, eventloop) = service::TrainService::new(&config);
    tokio::spawn(service::run_event_loop(service.clone(), eventloop));

    let app = routes::router(service);
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

    let result = axum::serve(listener, app)
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c().await.ok();
            tracing::info!("shutting down");
        })
        .await;

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("server error: {err}");
            ExitCode::FAILURE
        }
    }
}
