mod hubs;
pub mod players;
mod events;

use std::{env, fmt::Debug, io::Error, path::Path, str::FromStr, sync::Arc};
use players::Tank;
use serde::Deserialize;
use log::info;
use tokio::net::{TcpListener, TcpStream};
use crate::hubs::HubManager;



#[tokio::main]
async fn main() -> Result<(), Error> {
    let _ = env_logger::try_init();
    let hubs = HubManager::new().await;
    let listener = TcpListener::bind(&"127.0.0.1:8080".to_string()).await.expect("Failed to bind");
    info!("Listening on: http://localhost:8080/");
    while let Ok((stream, _)) = listener.accept().await {
        accept_connection(stream, &hubs).await;
    }

    Ok(())
}

async fn accept_connection(stream: TcpStream, hubs: &HubManager) {
    if let Ok(ws_stream) = tokio_tungstenite::accept_async(stream).await {
        hubs.create_client(ws_stream).await
    }
}

#[derive(Clone, Deserialize)]
pub struct Config {
    max_player_count: i32,
    map_size: f64,
    update_delay_ms: u64,
    tanks: Vec<Arc<Tank>>
}

pub fn get_env<T: FromStr>(name: &str) -> T where <T as FromStr>::Err: Debug {
    env::var(name).expect(format!("{name} env var not found").as_str()).parse().expect(format!("Invalid {name} env var type").as_str())
} 

impl Config {
    pub async fn get() -> Config {
        let data = tokio::fs::read(Path::new("../config.json")).await.expect("Error opening config");
        serde_json::from_slice(&data.as_slice()).expect("Error deserializing config")
    }
}