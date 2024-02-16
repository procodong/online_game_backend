use std::{ops::{Add, AddAssign}, sync::Arc};
use futures_util::{future, stream::{SplitSink, SplitStream}, SinkExt, StreamExt, TryStreamExt};
use log::warn;
use serde::Serialize;
use serde_json::value::RawValue;
use tokio::{net::TcpStream, sync::{broadcast, RwLock, RwLockWriteGuard}};
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
    messages: &mut broadcast::Receiver<Vec<u8>>) {
    while let Ok(message) = messages.recv().await {
        if let Err(e) = conn.send(tungstenite::Message::Binary(message)).await {
            warn!("Error sending data: {:?}", e);
        }
    }
}

pub async fn listen_for_messages(player: Arc<RwLock<Entity>>, read: SplitStream<WebSocketStream<TcpStream>>, hub: Arc<Hub>) {
    read.try_filter(|msg| future::ready(msg.is_text()))
    .for_each(|m| async {
        let text = m.unwrap().into_data();
        let event: EventData<&RawValue> = serde_json::from_slice(text.as_slice()).unwrap();
        let entity = &mut player.write().await;
        let result: Result<(), ()> = match event.event {
            Event::DirectionChange => change_direction(entity, event.data, &hub),
            Event::YawChange => change_yaw(entity, event.data, &hub),
            _ => Err(())
        };
        if result.is_err() {
            // kick client if they send an invalid request
            return;
        }
    }).await;
}

fn is_valid_degree(yaw: i32) -> bool {
    yaw <= 360 && yaw >= 0
}


fn change_direction<'a>(entity: &mut RwLockWriteGuard<'a, Entity>, direction_data: &RawValue, hub: &Arc<Hub>) -> Result<(), ()>{
    let direction: Result<DirectionChange, serde_json::Error> = serde_json::from_str(direction_data.get());
    if direction.is_err() {
        return Err(());
    }
    let direction = direction.unwrap();
    if !is_valid_degree(direction.direction) {
        return Err(());
    }
    let change = yaw_coordinate_change(direction.direction);
    entity.acceleration = Coordinates {
        x: change.x / 10.,
        y: change.y / 10.
    };
    hub.dispatch_event(&EventData {
        event: Event::MotionUpdate,
        data: entity.clone()
    });
    Ok(())
}

fn change_yaw<'a>(entity: &mut RwLockWriteGuard<'a, Entity>, yaw_raw_data: &RawValue, hub: &Arc<Hub>) -> Result<(), ()> {
    let yaw_update: Result<YawChange, serde_json::Error> = serde_json::from_str(yaw_raw_data.get());
    if yaw_update.is_err() {
        return Err(());
    }
    let yaw_update_data = yaw_update.unwrap();
    if !is_valid_degree(yaw_update_data.yaw) {
        return Err(());
    }
    entity.yaw = yaw_update_data.yaw;
    hub.dispatch_event(&EventData {
        event: Event::MotionUpdate,
        data: entity.clone()
    });
    Ok(())
}


pub struct Player {
    pub messages: broadcast::Sender<Vec<u8>>
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

    pub fn new(coords: Coordinates, id: Uuid) -> Self {
        Self {
            id,
            coordinates: coords,
            velocity: Coordinates::empty(),
            acceleration: Coordinates::empty(),
            size: 5.,
            yaw: 0
        }
    }

    pub fn move_once(&mut self) {
        self.coordinates += self.velocity.clone();
        self.velocity += self.acceleration.clone();
    }
}