use std::{array, ops::{Add, AddAssign}, sync::Arc};
use futures_util::{future, stream::{SplitSink, SplitStream}, SinkExt, StreamExt, TryStreamExt};
use log::{info, warn};
use serde::Serialize;
use serde_json::value::RawValue;
use tokio::{net::TcpStream, sync::{broadcast, RwLock}};
use tokio_tungstenite::WebSocketStream;
use tungstenite::Message;
use uuid::Uuid;

use crate::{events::{DirectionChange, Event, EventData, YawChange}, get_env, hubs::Hub};

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
        let mut entity =  player.write().await;
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

pub struct Player {
    pub messages: broadcast::Sender<Vec<u8>>
}

#[derive(Serialize, Clone)]
pub struct Entity {
    pub id: Uuid,
    // Position in the map
    pub coordinates: Coordinates,
    // Amount coordinates is changed when moved
    pub velocity: Coordinates,
    // Max amount of amount coordinates is user
    pub max_velocity: Coordinates,
    // Amount velocity is changed when moved
    pub acceleration: Coordinates,
    pub size: f32,
    pub yaw: i32,
    cannons: Vec<i32>,
    points: i32, 
    pub levels: [u8; 8]
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
            yaw: 0,
            levels: array::from_fn(|_| 0),
            cannons: Vec::new(),
            points: 0
        }
    }

    pub fn level(&self, stat: Stat) -> u8 {
        self.levels[stat as usize]
    }

    pub fn stat_multiplier(&self, stat: Stat) -> f32 {
        1. + self.level(stat) as f32 / 10.
    }

    pub fn increment_level(&mut self, stat: Stat) {
        let max_level: u8 = get_env("MAX_LEVEL");
        let current_level = self.level(stat.clone());
        if current_level < max_level {
            self.levels[stat as usize] += 1;
        }
    }

    fn velocity<F: Fn(&Coordinates) -> f64>(&self, axis: F) -> f64 {
        (axis(&self.max_velocity) - axis(&self.velocity)) / 10.
    }

    pub fn move_once(&mut self) {
        self.coordinates += self.velocity.clone();
        self.velocity = Coordinates {
            x: self.velocity(|c| c.x),
            y: self.velocity(|c| c.y)
        };
    }

    fn change_direction<T>(&mut self, direction: Result<DirectionChange, T>, hub: &Arc<Hub>) -> Result<(), ()>{
        let Ok(direction) = direction else {return Err(());};
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
            data: &self
        });
        Ok(())
    }
    
    fn change_yaw<T>(&mut self, yaw_update: Result<YawChange, T>, hub: &Arc<Hub>) -> Result<(), ()> {
        let Ok(yaw_update_data) = yaw_update else {return Err(());};
        if yaw_update_data.yaw == self.yaw {
            return Ok(());
        }
        self.yaw = yaw_update_data.yaw;
        hub.dispatch_event(&EventData {
            event: Event::MotionUpdate,
            data: &self
        });
        Ok(())
    }
}

#[derive(Clone)]
pub enum Stat {
    HealthRegen = 0,
    MaxHealth = 1,
    BodyDamage = 2,
    BulletSpeed = 3,
    BulletPenetration = 4,
    BulletDamage = 5,
    Reload = 6,
    MovementSpeed = 7
}

pub trait SendTick {
    fn tick(&mut self, tick: u64);
}