use std::sync::Arc;
use futures_util::{future, stream::{SplitSink, SplitStream}, SinkExt, StreamExt, TryStreamExt};
use log::{info, warn};
use serde::Serialize;
use serde_json::value::RawValue;
use tokio::{net::TcpStream, sync::{broadcast, Mutex, RwLock}};
use tokio_tungstenite::WebSocketStream;
use tungstenite::Message;
use uuid::Uuid;

use crate::events::{Event, EventData};

#[derive(Serialize, Clone)]
pub struct Coordinates {
    pub x: f32,
    pub y: f32
}
impl Coordinates {
    fn empty() -> Self {
        Self {
            x: 0., 
            y: 0.
        }
    }
}

pub async fn forward_messages_from_channel(
    conn: Box<Mutex<SplitSink<WebSocketStream<TcpStream>, Message>>>, 
    messages: Box<Mutex<broadcast::Receiver<String>>>) {
    loop {
        let mut source = messages.lock().await;
        let value = source.recv().await;
        if let Err(e) = value {
            warn!("Error receiving message for client");
            if let broadcast::error::RecvError::Closed = e {
                info!("No senders for event channel");
                return;
            }
            continue;
        }
        let data = value.unwrap();
        let mut ws = conn.lock().await;
        if let Err(e) = ws.send(tungstenite::Message::Text(data)).await {
            info!("Error sending data: {:?}", e);
        }
    }
}

pub async fn listen_for_messages(player: Arc<RwLock<Entity>>, read: SplitStream<WebSocketStream<TcpStream>>) {
    read.try_filter(|msg| future::ready(msg.is_text()))
    .for_each(|m| async {
        let text = m.unwrap().into_data();
        let data: EventData<&RawValue> = serde_json::from_slice(text.as_slice()).unwrap();
        match data.event {
            Event::MotionUpdate => {}
            Event::EntityCreate => {}
        }
    }).await;
}


pub struct Player {
    pub messages: broadcast::Sender<String>
}

#[derive(Serialize, Clone)]
pub struct Entity {
    pub id: Uuid,
    pub coordinates: Coordinates,
    pub velocity: Coordinates,
    pub acceleration: Coordinates,
    pub size: f32,
    pub yaw: i32
}

impl Entity {

    pub fn new(coords: Coordinates) -> Self {
        Self {
            id: Uuid::new_v4(),
            coordinates: coords,
            velocity: Coordinates::empty(),
            acceleration: Coordinates::empty(),
            size: 5.,
            yaw: 0
        }
    }
    pub fn move_once(&mut self) {
        self.coordinates.x += self.velocity.x;
        self.coordinates.y += self.velocity.y;
        self.velocity.x += self.acceleration.x;
        self.velocity.y += self.acceleration.y;
    }
}