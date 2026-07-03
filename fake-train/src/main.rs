mod app;
mod belief;
mod config;
mod mqtt;
mod sim;
mod track;
mod ui;

use std::process::ExitCode;

use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> ExitCode {
    dotenvy::dotenv().ok();

    // config errors must print normally, so resolve before entering the TUI
    let config = match config::Config::from_env() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("configuration error: {err}");
            return ExitCode::FAILURE;
        }
    };
    let broker = format!("mqtt://{}:{}", config.mqtt_host, config.mqtt_port);

    let (client, eventloop) = mqtt::connect(&config);
    let (tx, rx) = mpsc::channel(64);
    tokio::spawn(mqtt::run(client.clone(), eventloop, tx));

    let terminal = ratatui::init(); // installs a panic hook that restores the terminal
    let result = app::App::new(client, rx, broker).run(terminal).await;
    ratatui::restore();

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("TUI error: {err}");
            ExitCode::FAILURE
        }
    }
}
