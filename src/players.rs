use std::{array, sync::Arc, usize};
use futures_util::{SinkExt, StreamExt};
use log::warn;
use serde::{Deserialize, Serialize};
use tokio::{net::TcpStream, sync::mpsc};
use tokio_tungstenite::WebSocketStream;
use tungstenite::{protocol::CloseFrame, Message};

use crate::{events::{DirectionChange, UserEvent, UserMessage}, hubs::Id, Config};

#[derive(Serialize, Clone, Debug)]
pub struct Vector {
    pub x: f64,
    pub y: f64
}

impl Vector {
    pub fn empty() -> Self {
        Self {
            x: 0., 
            y: 0.
        }
    }

    pub fn cap(&mut self, max: &Vector) -> &mut Self {
        if self.x.abs() > max.x.abs() {
            self.x = max.x;
        } if self.y.abs() > max.y.abs() {
            self.y = max.y;
        }
        self
    }

    pub fn combine(&mut self, other: &Vector) -> &mut Self {
        self.x += other.x;
        self.y += other.y;
        self
    }
}

pub fn yaw_coordinate_change(yaw: i32) -> Vector {
    let radians = yaw as f64 * std::f64::consts::PI / 180.;
    Vector { 
        x: radians.sin(), 
        y: radians.cos() 
    }
}

pub async fn handle_client_connection(mut conn: WebSocketStream<TcpStream>, mut messages: mpsc::Receiver<Message>, updates: mpsc::Sender<UserMessage>, id: Id) {
    let close_value = loop {
        tokio::select! {
            incoming_message = conn.next() => {
                if let Some(close) = handle_message(incoming_message, &updates, id, &mut conn).await {
                    break close;
                }
            }
            sent_message = messages.recv() => {
                let Some(message) = sent_message else {
                    break None;
                };
                if let Err(_) = conn.send(message).await {
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
    incoming_message: Option<Result<Message, 
    tungstenite::error::Error>>, 
    updates: &mpsc::Sender<UserMessage>, id: Id, 
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
        Message::Ping(ping) => {let _ = conn.send(Message::Pong(ping.to_vec())).await;},
        _ => {}
    };
    None
}

#[derive(Debug)]
pub struct Entity {
    pub id: Id,
    // Position in the map
    pub coordinates: Vector,
    // Amount coordinates is changed when moved
    pub velocity: Vector,
    // Max amount of amount coordinates is user
    pub max_velocity: Vector,
    // Amount velocity is changed when moved
    pub acceleration: Vector,
    pub target_yaw: i32,
    pub yaw: i32,
    pub tank: Arc<Tank>,
    levels: [u8; 8],
    stats: EntityType,
    pub shooting: bool
}

impl Entity {

    pub fn new(coords: Vector, id: Id, config: &Config, inner: EntityType) -> Self {
        Self {
            id,
            coordinates: coords,
            velocity: Vector::empty(),
            max_velocity: Vector::empty(),
            acceleration: Vector::empty(),
            yaw: 0,
            target_yaw: 0,
            levels: array::from_fn(|_| 0),
            tank: config.tanks[0].clone(),
            stats: inner,
            shooting: false
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

    fn stat(&self, stat: Stat) -> i32 {
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

    pub fn create_bullet(&self, cannon: &Cannon, id: i32) -> Self {
        let yaw = self.yaw + cannon.yaw;
        let direction = yaw_coordinate_change(yaw);
        let bullet = EntityType::Bullet(Bullet { author: self.id });
        Entity {
            id,
            coordinates: self.coordinates.clone(),
            velocity: direction.clone(),
            max_velocity: Vector { x: 0., y: 0. },
            acceleration: Vector { x: direction.x / 10., y: direction.y / 10. },
            yaw,
            target_yaw: yaw,
            tank: cannon.bullet.clone(),
            levels: array::from_fn(|i| {
                match Stat::for_child(i) {
                    Some(s) => self.level(s),
                    _ => 0
                }
            }),
            stats: bullet,
            shooting: false
        }
    }

    pub fn tick(&mut self) {
        if self.yaw != self.target_yaw {
            self.update_yaw();
        }
    }

    fn update_yaw(&mut self) {
        if self.yaw < self.target_yaw {
            self.yaw += 1;
        } else {
            self.yaw -= 1;
        }
    }

    pub fn move_once(&mut self) {
        self.coordinates.combine(&self.velocity);
        self.velocity.combine(&self.acceleration).cap(&self.max_velocity);
    }

    fn change_direction(&mut self, direction: DirectionChange) {
        let velocity = direction.to_velocity();
        let acceleration_x = velocity.x / 10.;
        let acceleration_y = velocity.y / 10.;
        self.acceleration = Vector {
            x: if acceleration_x != 0. {acceleration_x} else {-self.max_velocity.x / 10.},
            y: if acceleration_y != 0. {acceleration_y} else {-self.max_velocity.y / 10.}
        };
        self.max_velocity = velocity;
    }

    pub fn handle_event(&mut self, event: UserEvent) {
        match event {
            UserEvent::DirectionChange(d) => self.change_direction(d),
            UserEvent::Yaw(yaw) => self.target_yaw = yaw,
            UserEvent::SetShooting(shooting) => self.shooting = shooting,
            UserEvent::LevelUpgrade(stat) => self.increment_level(stat)
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
    pub yaw: i32,
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