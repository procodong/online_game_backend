pub mod hubs;
pub mod players;
mod events;

use std::{env, io::Error};
use tungstenite::Message::Text;
use futures_util::{future, SinkExt, StreamExt, TryStreamExt};
use log::info;
use tokio::net::{TcpListener, TcpStream};

#[tokio::main]
async fn main() -> Result<(), Error> {
    let _ = env_logger::try_init();
    let addr = env::args().nth(1).unwrap_or_else(|| "127.0.0.1:8080".to_string());

    let listener = TcpListener::bind(&addr).await.expect("Failed to bind");
    info!("Listening on: {}", addr);
    println!("Listening on {addr}");
    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(accept_connection(stream));
    }

    Ok(())
}

async fn accept_connection(stream: TcpStream) {
    let addr = stream.peer_addr().expect("connected streams should have a peer address");
    info!("Peer address: {}", addr);

    let ws_stream = tokio_tungstenite::accept_async(stream).await.expect("Error during the websocket handshake occurred");
    info!("New WebSocket connection: {}", addr);
    let (mut write, read) = ws_stream.split();
    
    write.send(Text(String::from("hello world."))).await.expect("Messed up man");
    read.try_filter(|msg| future::ready(msg.is_text() || msg.is_binary()))
        .for_each(|m|
            future::ready(
                println!("{}", m.unwrap().into_text().unwrap())
            ))
            .await;
}

#[derive(Clone)]
pub struct Config {
    max_player_count: i32,
    event_distance: i32,
    min_tile_size: i32,
    max_tile_player_count: i32,
    map_size: f32,
    update_delay_ms: u64
}
