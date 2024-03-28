mod hubs;
mod players;
mod events;

use std::{io::Error, path::Path, sync::Arc};
use players::Tank;
use serde::{Deserialize, Serialize};
use log::{info, warn};
use tokio::net::TcpListener;
use crate::hubs::HubManager;


#[tokio::main]
async fn main() -> Result<(), Error> {
    env_logger::try_init().expect("Failed to init logger");
    let mut hubs = HubManager::new().await;
    let listener = TcpListener::bind(&"127.0.0.1:8080".to_string()).await.expect("Failed to bind");
    info!("Listening on: http://localhost:8080/");
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                if let Ok(ws_stream) = tokio_tungstenite::accept_async(stream).await {
                    hubs.create_client(ws_stream).await;
                }
            },
            Err(e) => warn!("Error receiving request: {e:?}")
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Config {
    max_player_count: i32,
    map_size: f64,
    update_delay_ms: u64,
    tanks: Vec<Arc<Tank>>,
    hit_delay: u32
}

impl Config {
    pub async fn get() -> Config {
        let data = tokio::fs::read(Path::new("../config.json")).await.expect("Error opening config");
        serde_json::from_slice(&data.as_slice()).expect("Error deserializing config")
    }
}