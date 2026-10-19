use std::time::Duration;

use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, QoS};
use shotexpress_protocol::TOPIC_COMMAND;
use tokio::sync::mpsc;

use crate::config::Config;

const RECONNECT_PERIOD: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub enum MqttIn {
    Connected,
    Disconnected(String),
    Command(Vec<u8>),
}

pub fn connect(cfg: &Config) -> (AsyncClient, EventLoop) {
    let mut options = MqttOptions::new(
        cfg.mqtt_client_id.clone(),
        cfg.mqtt_host.clone(),
        cfg.mqtt_port,
    );
    options.set_keep_alive(Duration::from_secs(60));
    options.set_clean_session(true);
    if let (Some(username), Some(password)) = (&cfg.mqtt_username, &cfg.mqtt_password) {
        options.set_credentials(username.clone(), password.clone());
    }
    AsyncClient::new(options, 64)
}

pub async fn run(client: AsyncClient, mut eventloop: EventLoop, tx: mpsc::Sender<MqttIn>) {
    loop {
        match eventloop.poll().await {
            Ok(Event::Incoming(Packet::ConnAck(_))) => {
                // clean_session drops subscriptions; resubscribe on every
                // ConnAck. Spawned because the request channel may still hold
                // a backlog that only this eventloop can drain — awaiting
                // subscribe() inline would deadlock, and try_subscribe would
                // fail silently, leaving the sim deaf to commands.
                let subscribe_client = client.clone();
                let subscribe_tx = tx.clone();
                tokio::spawn(async move {
                    if let Err(err) = subscribe_client
                        .subscribe(TOPIC_COMMAND, QoS::AtLeastOnce)
                        .await
                    {
                        let _ = subscribe_tx
                            .send(MqttIn::Disconnected(format!("subscribe failed: {err}")))
                            .await;
                    }
                });
                if tx.send(MqttIn::Connected).await.is_err() {
                    return;
                }
            }
            Ok(Event::Incoming(Packet::Publish(publish))) => {
                if publish.topic == TOPIC_COMMAND
                    && tx
                        .send(MqttIn::Command(publish.payload.to_vec()))
                        .await
                        .is_err()
                {
                    return;
                }
            }
            Ok(_) => {}
            Err(err) => {
                if tx
                    .send(MqttIn::Disconnected(err.to_string()))
                    .await
                    .is_err()
                {
                    return;
                }
                tokio::time::sleep(RECONNECT_PERIOD).await;
            }
        }
    }
}
