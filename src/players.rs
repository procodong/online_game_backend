use std::{fmt::Debug, ops::{Add, AddAssign}, sync::Arc};
use futures_util::{future, stream::{SplitSink, SplitStream}, SinkExt, StreamExt, TryStreamExt};
use log::{info, warn};
use serde::Serialize;
use serde_json::value::RawValue;
use tokio::{net::TcpStream, sync::{broadcast, RwLock}};
use tokio_tungstenite::WebSocketStream;
use tungstenite::Message;
use uuid::Uuid;

use crate::{events::{DirectionChange, Event, EventData, YawChange}, hubs::Hub};

#[derive(Serialize, Clone)]
pub struct Coordinates {
    pub x: f64,
    pub y: f64
}
impl Coordinates {
    fn empty() -> Self {
        Self {
            x: 0., 
            y: 0.
        }
    }
}

impl Add for Coordinates {
    type Output = Coordinates;
    fn add(self, rhs: Self) -> Self::Output {
        Coordinates {
            x: self.x + rhs.x,
            y: self.y + rhs.y
        }
    }
}

impl AddAssign for Coordinates {
    fn add_assign(&mut self, rhs: Self) {
        self.x += rhs.x;
        self.y += rhs.y;
    }
}


pub fn yaw_coordinate_change(yaw: i32) -> Coordinates {
    let radians = yaw as f64 * std::f64::consts::PI / 180.;
    Coordinates { 
        x: radians.sin(), 
        y: radians.cos() 
    }
}


pub async fn forward_messages_from_channel(
    conn: &mut SplitSink<WebSocketStream<TcpStream>, Message>, 
    messages: &mut broadcast::Receiver<Vec<u8>>
) {
    while let Ok(message) = messages.recv().await {
        if let Err(e) = conn.send(tungstenite::Message::Binary(message)).await {
            warn!("Error sending data: {:?}", e);
        }
    }
    if let Err(e) = conn.close().await {
        warn!("Error closing connection: {:?}", e);
    } else {
        info!("Closed connection succesfully");
    }
}

pub async fn listen_for_messages(player: Arc<RwLock<Entity>>, read: SplitStream<WebSocketStream<TcpStream>>, hub: Arc<Hub>) {
    read.try_filter(|msg| future::ready(msg.is_text()))
    .for_each(|m| async {
        if m.is_err() {
            return;
        }
        let text = m.unwrap().into_data();
        let event: Result<EventData<&RawValue>, _> = serde_json::from_slice(text.as_slice());
        let entity = &mut player.write().await;
        if event.is_err() {
            hub.kick_player(entity.id);
            return;
        }
        let event = event.unwrap();
        let result: Result<(), ()> = match event.event {
            Event::DirectionChange => entity.change_direction(serde_json::from_str(event.data.get()) , &hub),
            Event::YawChange => entity.change_yaw(serde_json::from_str(event.data.get()), &hub),
            _ => Err(())
        };
        if result.is_err() {
            hub.kick_player(entity.id);
        }
    }).await;
}

fn is_valid_degree(yaw: i32) -> bool {
    yaw <= 360 && yaw >= 0
}


pub struct Player {
    pub messages: broadcast::Sender<Vec<u8>>
}

#[derive(Serialize, Clone)]
pub struct Entity {
    pub id: Uuid,
    pub coordinates: Coordinates,
    pub velocity: Coordinates,
    pub max_velocity: Coordinates,
    pub acceleration: Coordinates,
    pub size: f32,
    pub yaw: i32
}

impl Entity {

    pub fn new(coords: Coordinates, id: Uuid) -> Self {
        Self {
            id,
            coordinates: coords,
            velocity: Coordinates::empty(),
            max_velocity: Coordinates::empty(),
            acceleration: Coordinates::empty(),
            size: 5.,
            yaw: 0
        }
    }

    pub fn move_once(&mut self) {
        self.coordinates += self.velocity.clone();
        let velo = self.velocity.clone() + self.acceleration.clone();
        if velo.x.abs() > self.max_velocity.x.abs() || velo.y.abs() > self.max_velocity.y.abs() {
            self.velocity = self.max_velocity.clone();
        } else {
            self.velocity = velo;
        }
    }
    fn change_direction<'a, T: Debug>(&mut self, direction: Result<DirectionChange, T>, hub: &Arc<Hub>) -> Result<(), ()>{
        if direction.is_err() {
            return Err(());
        }
        let direction = direction.unwrap();
        // if down is true 1 if up is true -1
        let x_mod = direction.down as i32 - direction.up as i32; 
        let y_mod = direction.right as i32 - direction.left as i32;
        self.acceleration = Coordinates {
            x: x_mod as f64 / 10.,
            y: y_mod as f64 / 10.
        };
        self.max_velocity = Coordinates {
            x: x_mod as f64,
            y: y_mod as f64
        };
        hub.dispatch_event(&EventData {
            event: Event::MotionUpdate,
            data: self.clone()
        });
        Ok(())
    }
    
    fn change_yaw<'a, T: Debug>(&mut self, yaw_update: Result<YawChange, T>, hub: &Arc<Hub>) -> Result<(), ()> {
        if yaw_update.is_err() {
            return Err(());
        }
        let yaw_update_data = yaw_update.unwrap();
        if !is_valid_degree(yaw_update_data.yaw) {
            return Err(());
        }
        self.yaw = yaw_update_data.yaw;
        hub.dispatch_event(&EventData {
            event: Event::MotionUpdate,
            data: self.clone()
        });
        Ok(())
    }
}
