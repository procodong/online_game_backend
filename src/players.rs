use std::{array, ops::{Add, AddAssign}, sync::Arc};
use futures_util::{future, stream::{SplitSink, SplitStream}, SinkExt, StreamExt, TryStreamExt};
use log::{info, warn};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use tokio::{net::TcpStream, sync::{broadcast, RwLock}};
use tokio_tungstenite::WebSocketStream;
use tungstenite::Message;
use uuid::Uuid;

use crate::{events::{DirectionChange, Event, EventData, YawChange}, hubs::Hub, Config};

#[derive(Serialize, Clone, Debug)]
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

    pub fn min(&mut self, other: &Coordinates) -> &mut Self {
        if self.x < other.x {
            self.x = other.x;
        } if self.y < other.y {
            self.y = other.y;
        }
        self
    }

    pub fn cap(&mut self, max: &Coordinates) -> &mut Self {
        if self.x > max.x {
            self.x = max.x;
        } if self.y > max.y {
            self.y = max.y;
        }
        self
    }

    pub fn combine(&mut self, other: &Coordinates) -> &mut Self {
        self.x += other.x;
        self.y += other.y;
        self
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
    messages: &mut broadcast::Receiver<Message>
) {
    while let Ok(message) = messages.recv().await {
        if let Err(e) = conn.send(message).await {
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
            Event::DirectionChange => entity.direction_change_event(serde_json::from_str(event.data.get()) , &hub),
            Event::YawChange => entity.yaw_change_event(serde_json::from_str(event.data.get()), &hub),
            _ => Err(())
        };
        if result.is_err() {
            hub.kick_player(entity.id);
        }
    }).await;
}

#[derive(Serialize, Clone, Debug)]
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
    pub tank: Arc<Tank>,
    pub levels: [u8; 8],
    pub stats: EntityType
}

impl Entity {

    pub fn new(coords: Coordinates, id: Uuid, config: &Config, inner: EntityType) -> Self {
        Self {
            id,
            coordinates: coords,
            velocity: Coordinates::empty(),
            max_velocity: Coordinates::empty(),
            acceleration: Coordinates::empty(),
            size: 5.,
            yaw: 0,
            levels: array::from_fn(|_| 0),
            tank: config.tanks[0].clone(),
            stats: inner
        }
    }

    pub fn level(&self, stat: Stat) -> u8 {
        self.levels[stat as usize]
    }

    pub fn stat_multiplier(&self, stat: Stat) -> f32 {
        match stat {
            Stat::Reload => 1. - self.level(stat) as f32 / 20.,
            _ => 1. + self.level(stat) as f32 / 10.
        }
    }

    fn base_stat(&self, stat: Stat) -> i32 {
        self.tank.base_stats[stat as usize]
    }

    fn stat(&self, stat: Stat) -> i32 {
        (self.stat_multiplier(stat.clone()) * self.base_stat(stat) as f32) as i32
    }
    
    pub fn active_cannons(&self, tick: u32) -> impl Iterator<Item = &Cannon> {
        let speed =  self.stat(Stat::Reload);
        self.tank.cannons.iter().filter(move |c| c.delay * speed % tick as i32 == 0)
    }

    const MAX_LEVEL: u8 = 10;

    // This function shouldn't be accessible for a non-player entity
    // Oh well!
    pub fn increment_level(&mut self, stat: Stat) {
        let current_level = self.level(stat.clone());
        if current_level + 1 >= Self::MAX_LEVEL {
            return;
        }
        if let EntityType::Player(p) = &mut self.stats {
            if p.points == 0 {
                return;
            }
            p.points -= 1;
            self.levels[stat as usize] += 1;
        }
    }

    pub fn move_once(&mut self) {
        self.coordinates.combine(&self.velocity);
        self.velocity.combine(&self.acceleration).cap(&self.max_velocity);
    }

    fn direction_change_event(&mut self, direction: serde_json::Result<DirectionChange>, hub: &Arc<Hub>) -> Result<(), ()> {
        let Ok(direction) = direction else {return Err(());};
        self.change_direction(direction);
        hub.dispatch_event(&EventData {
            event: Event::MotionUpdate,
            data: &self
        });
        Ok(())
    }

    pub fn change_direction(&mut self, direction: DirectionChange) {
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
    }

    fn yaw_change_event(&mut self, yaw_update: serde_json::Result<YawChange>, hub: &Arc<Hub>) -> Result<(), ()> {
        let Ok(yaw_update_data) = yaw_update else {return Err(());};
        self.change_yaw(yaw_update_data);
        hub.dispatch_event(&EventData {
            event: Event::MotionUpdate,
            data: &self
        });
        Ok(())
    }
    
    fn change_yaw(&mut self, yaw_update: YawChange) {
        if yaw_update.yaw == self.yaw {
            return;
        }
        self.yaw = yaw_update.yaw;
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

#[derive(Debug, Clone, Deserialize)]
pub struct Cannon {
    pub yaw: i32,
    pub delay: i32,
    pub size: i32
}

#[derive(Debug, Clone, Deserialize)]
pub struct Tank {
    pub cannons: Vec<Cannon>,
    pub base_stats: [i32; 8],
    pub size: f64,
    pub id: i32
}

impl Serialize for Tank {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_i32(self.id)
    }
}

#[derive(Debug, Clone, Serialize)]
pub enum EntityType {
    Player(Player),
    Bullet(Bullet)
}

#[derive(Serialize, Debug, Clone)]
pub struct Player {
    pub points: i32,
    pub score: i32
}

#[derive(Debug, Serialize, Clone)]
pub struct Bullet {
    pub author: Uuid
}