pub mod hubs;
pub mod players;
mod events;

use std::{env, io::Error, sync::Arc};
use serde::Deserialize;
use log::info;
use tokio::net::{TcpListener, TcpStream};

use crate::hubs::HubManager;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let _ = env_logger::try_init();
    let addr = env::args().nth(1).unwrap_or_else(|| "127.0.0.1:8080".to_string());
    let hubs = Arc::new(HubManager::new().await);
    let listener = TcpListener::bind(&addr).await.expect("Failed to bind");
    info!("Listening on: {}", addr);
    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(accept_connection(stream, hubs.clone()));
    }

    Ok(())
}

async fn accept_connection(stream: TcpStream, hubs: Arc<HubManager>) {
    let addr = stream.peer_addr().expect("connected streams should have a peer address");

    let ws_stream = tokio_tungstenite::accept_async(stream).await.expect("Error during the websocket handshake occurred");
    info!("New WebSocket connection: {}", addr);
    hubs.create_client(ws_stream).await;
}

#[derive(Clone, Deserialize)]
pub struct Config {
    max_player_count: i32,
    map_size: f64,
    update_delay_ms: u64
}

impl Config {
    pub async fn get() -> Config {
        let config = tokio::fs::read("config.json").await.expect("Error reading config");
        serde_json::from_slice(&config).expect("Error deserializing config")
    }
}