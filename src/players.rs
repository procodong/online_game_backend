use std::{array, sync::Arc, usize};
use futures_util::{SinkExt, StreamExt};
use log::warn;
use serde::{Deserialize, Serialize};
use tokio::{net::TcpStream, sync::{broadcast, mpsc}};
use tokio_tungstenite::WebSocketStream;
use tungstenite::{protocol::CloseFrame, Message};

use crate::{events::{DirectionChange, UserEvent, UserMessage}, hubs::Id, Config};

#[derive(Serialize, Clone, Debug, PartialEq, PartialOrd)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64
}

impl Vec2 {
    pub fn empty() -> Self {
        Self {
            x: 0., 
            y: 0.
        }
    }

    pub fn cap(&mut self, max: &Vec2) -> &mut Self {
        if self.x.abs() > max.x.abs() {
            self.x = max.x;
        } if self.y.abs() > max.y.abs() {
            self.y = max.y;
        }
        self
    }

    #[inline]
    pub fn add(&mut self, other: &Vec2) -> &mut Self {
        self.x += other.x;
        self.y += other.y;
        self
    }
}

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Debug)]
pub struct Yaw(i16);

impl Yaw {
    fn to_vec(&self) -> Vec2 {
        let radians = self.0 as f64 * std::f64::consts::PI / 180.;
        Vec2 { 
            x: radians.sin(), 
            y: radians.cos() 
        }
    }
}

pub async fn handle_client_connection(mut conn: WebSocketStream<TcpStream>, mut messages: broadcast::Receiver<Vec<u8>>, updates: mpsc::Sender<UserMessage>, id: Id) {
    let close_value = loop {
        tokio::select! {
            incoming_message = conn.next() => {
                if let Some(close) = handle_message(incoming_message, &updates, id, &mut conn).await {
                    break close;
                }
            }
            sent_message = messages.recv() => {
                let Ok(message) = sent_message else {
                    break None;
                };
                if let Err(_) = conn.send(Message::Binary(message)).await {
                    break None;
                }
            }
        };
    };
    if let Err(e) = conn.close(close_value).await {
        warn!("Error closing connection {:?}", e);
    }
    let _ = updates.send(UserMessage::GoingAway(id)).await;
}

async fn handle_message<'a>(
    incoming_message: Option<Result<Message, tungstenite::error::Error>>, 
    updates: &mpsc::Sender<UserMessage>, 
    id: Id, 
    conn: &mut WebSocketStream<TcpStream>) -> Option<Option<CloseFrame<'a>>> {
    let Some(Ok(message)) = incoming_message else {
        return Some(None);
    };
    match message {
        Message::Binary(binary) => {
            let Ok(event) = bincode::deserialize(binary.as_slice()) else {
                return Some(None);
            };
            if let Err(_) = updates.send(UserMessage::Event {
                event,
                user: id
            }).await {
                return Some(None);
            }
        },
        Message::Close(close) => return Some(close),
        Message::Ping(ping) => {
            let _ = conn.send(Message::Pong(ping.to_vec())).await;
        },
        _ => {}
    };
    None
}

#[derive(Debug)]
pub struct Entity {
    pub coordinates: Vec2,
    pub velocity: Vec2,
    acceleration: Vec2,
    max_velocity: Vec2,
    pub yaw: Yaw,
    pub tank: Arc<Tank>,
    levels: [u8; 8],
    stats: EntityType,
    pub shooting: bool,
    health: i16
}

impl Entity {

    pub fn new(coords: Vec2, config: &Config, inner: EntityType) -> Self {
        Self {
            coordinates: coords,
            velocity: Vec2::empty(),
            max_velocity: Vec2::empty(),
            acceleration: Vec2::empty(),
            yaw: Yaw(0),
            levels: array::from_fn(|_| 0),
            tank: config.tanks[0].clone(),
            stats: inner,
            shooting: false,
            health: 100
        }
    }

    fn level(&self, stat: Stat) -> u8 {
        self.levels[stat as usize]
    }

    fn stat_multiplier(&self, stat: Stat) -> f32 {
        match stat {
            Stat::Reload => 1. - self.level(stat) as f32 / 20.,
            _ => 1. + self.level(stat) as f32 / 10.
        }
    }

    fn base_stat(&self, stat: Stat) -> i32 {
        self.tank.base_stats[stat as usize]
    }

    pub fn stat(&self, stat: Stat) -> i32 {
        (self.stat_multiplier(stat.clone()) * self.base_stat(stat) as f32) as i32
    }
    
    pub fn active_cannons(&self, tick: u32) -> impl Iterator<Item = &Cannon> {
        let speed =  self.stat(Stat::Reload);
        self.tank.cannons.iter().filter(move |c| c.delay * speed % tick as i32 == 0)
    }

    const MAX_LEVEL: u8 = 10;

    pub fn increment_level(&mut self, stat: Stat) {
        let current_level = self.level(stat.clone());
        if current_level + 1 >= Self::MAX_LEVEL {
            return;
        }
        if let EntityType::Player(p) = &mut self.stats {
            if p.points <= 0 {
                return;
            }
            p.points -= 1;
            self.levels[stat as usize] += 1;
        }
    }

    pub fn create_bullet(&self, cannon: &Cannon, own_id: Id) -> Self {
        let yaw = Yaw(self.yaw.0 + cannon.yaw);
        let direction = yaw.to_vec();
        let bullet = EntityType::Bullet(Bullet { author: own_id });
        Entity {
            coordinates: self.coordinates.clone(),
            velocity: direction.clone(),
            max_velocity: Vec2::empty(),
            acceleration: Vec2 { x: direction.x / 10., y: direction.y / 10. },
            yaw,
            tank: cannon.bullet.clone(),
            levels: array::from_fn(|i| {
                match Stat::for_child(i) {
                    Some(s) => self.level(s),
                    _ => 0
                }
            }),
            stats: bullet,
            shooting: false,
            health: 100
        }
    }

    pub fn update_movement(&mut self, max: f64) {
        self.coordinates.add(&self.velocity).cap(&Vec2 { x: max, y: max });
        self.velocity.add(&self.acceleration).cap(&self.max_velocity);
    }

    pub fn damage(&mut self, damage: i32) -> bool {
        let max_health = self.stat(Stat::MaxHealth);
        let health_change = damage as f32 / max_health as f32 * 100.;
        self.health -= health_change as i16;
        self.health > 0
    }

    pub fn distance_from(&self, other: &Entity) -> f64 {
        ((self.coordinates.x - other.coordinates.x).powi(2) + (self.coordinates.y - other.coordinates.y).powi(2)).sqrt()
    }

    fn change_direction(&mut self, direction: DirectionChange) {
        let velocity = direction.to_vec();
        let acceleration_x = velocity.x / 10.;
        let acceleration_y = velocity.y / 10.;
        self.acceleration = Vec2 {
            x: if acceleration_x != 0. {acceleration_x} else {-self.max_velocity.x / 10.},
            y: if acceleration_y != 0. {acceleration_y} else {-self.max_velocity.y / 10.}
        };
        self.max_velocity = velocity;
    }

    pub fn handle_event(&mut self, event: UserEvent) {
        match event {
            UserEvent::DirectionChange { direction } => self.change_direction(direction),
            UserEvent::Yaw { yaw } => self.yaw = yaw,
            UserEvent::SetShooting { shooting } => self.shooting = shooting,
            UserEvent::LevelUpgrade { stat } => self.increment_level(stat)
        };
    }
}

#[derive(Clone, Deserialize)]
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

impl Stat {
    fn for_child(value: usize) -> Option<Self> {
        match value {
            5 /*bulled damage */ => Some(Self::BodyDamage),
            4 /*bullet penetration */ => Some(Self::MaxHealth),
            3 /*bullet speed */ => Some(Self::MovementSpeed),
            _ => None
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Cannon {
    pub yaw: i16,
    pub delay: i32,
    pub size: i32,
    pub bullet: Arc<Tank>
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
    pub author: Id
}