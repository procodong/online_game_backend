pub mod hubs;
pub mod players;
mod events;

use std::{env, fmt::Debug, io::Error, str::FromStr, sync::Arc};
use serde::Deserialize;
use log::info;
use tokio::net::{TcpListener, TcpStream};

use crate::hubs::HubManager;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let _ = env_logger::try_init();
    let hubs = Arc::new(HubManager::new());
    let listener = TcpListener::bind(&"127.0.0.1:8080".to_string()).await.expect("Failed to bind");
    info!("Listening on: http://localhost:8080/");
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

pub fn get_env<T: FromStr>(name: &str) -> T where <T as FromStr>::Err: Debug {
    env::var(name).expect(format!("{name} env var not found").as_str()).parse().expect(format!("Invalid {name} env var type").as_str())
} 

impl Config {
    pub fn get() -> Config {
        Config {
            max_player_count: get_env("MAX_PLAYER_COUNT"),
            map_size: get_env("MAP_SIZE"),
            update_delay_ms: get_env("UPDATE_DELAY_MS")
        }
    }
}