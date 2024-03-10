mod hubs;
mod players;
mod events;

use std::{io::Error, path::Path, sync::Arc};
use players::Tank;
use serde::Deserialize;
use log::info;
use tokio::net::TcpListener;
use crate::hubs::HubManager;


#[tokio::main]
async fn main() -> Result<(), Error> {
    let _ = env_logger::try_init();
    let mut hubs = HubManager::new().await;
    let listener = TcpListener::bind(&"127.0.0.1:8080".to_string()).await.expect("Failed to bind");
    info!("Listening on: http://localhost:8080/");
    while let Ok((stream, _)) = listener.accept().await {
        if let Ok(ws_stream) = tokio_tungstenite::accept_async(stream).await {
            hubs.create_client(ws_stream).await
        }
    }

    Ok(())
}

#[derive(Clone, Deserialize)]
pub struct Config {
    max_player_count: i32,
    map_size: f64,
    update_delay_ms: u64,
    tanks: Vec<Arc<Tank>>
}

impl Config {
    pub async fn get() -> Config {
        let data = tokio::fs::read(Path::new("../config.json")).await.expect("Error opening config");
        serde_json::from_slice(&data.as_slice()).expect("Error deserializing config")
    }
}