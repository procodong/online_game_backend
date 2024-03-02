use std::{array, sync::Arc, usize};
use futures_util::{future, stream::{SplitSink, SplitStream}, SinkExt, StreamExt, TryStreamExt};
use log::{info, warn};
use serde::{Deserialize, Serialize};
use tokio::{net::TcpStream, sync::{broadcast, RwLock}};
use tokio_tungstenite::WebSocketStream;
use tungstenite::Message;
use uuid::Uuid;

use crate::{events::{DirectionChange, Event, EventData, UserEvent}, hubs::Hub, Config};

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
        if self.x.abs() < other.x.abs() {
            self.x = other.x;
        } if self.y.abs() < other.y.abs() {
            self.y = other.y;
        }
        self
    }

    pub fn cap(&mut self, max: &Coordinates) -> &mut Self {
        if self.x.abs() > max.x.abs() {
            self.x = max.x;
        } if self.y.abs() > max.y.abs() {
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
    .for_each(|msg| async {
        if msg.is_err() {
            return;
        }
        let mut entity = player.write().await;
        if entity.handle_event(msg.unwrap(), &hub).is_err() {
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
    pub yaw: i32,
    pub tank: Arc<Tank>,
    pub levels: [u8; 8],
    pub stats: EntityType,
    shooting: bool
}

impl Entity {

    pub fn new(coords: Coordinates, id: Uuid, config: &Config, inner: EntityType) -> Self {
        Self {
            id,
            coordinates: coords,
            velocity: Coordinates::empty(),
            max_velocity: Coordinates::empty(),
            acceleration: Coordinates::empty(),
            yaw: 0,
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

    // This function shouldn't be accessible for a non-player entity
    // Oh well!
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

    fn create_bullet(&self, cannon: &Cannon) -> Self {
        let yaw = self.yaw + cannon.yaw;
        let direction = yaw_coordinate_change(yaw);
        let bullet = EntityType::Bullet(Bullet { author: self.id });
        Entity {
            id: Uuid::new_v4(),
            coordinates: self.coordinates.clone(),
            velocity: direction.clone(),
            max_velocity: Coordinates { x: 0., y: 0. },
            acceleration: Coordinates { x: direction.x / 10., y: direction.y / 10. },
            yaw,
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

    pub fn tick(&self, tick: u32, hub: &Arc<Hub>) {
        for cannon in self.active_cannons(tick) {
            let entity = self.create_bullet(cannon);
            hub.spawn_entity(entity);
        }
    }

    pub fn move_once(&mut self) {
        self.coordinates.combine(&self.velocity);
        self.velocity.combine(&self.acceleration).cap(&self.max_velocity);
    }

    fn direction_change_event(&mut self, direction: DirectionChange, hub: &Arc<Hub>) {
        self.change_direction(direction);
        hub.dispatch_event(EventData {
            event: Event::EntityUpdate,
            data: &self
        });
    }

    fn handle_event(&mut self, message: Message, hub: &Arc<Hub>) -> Result<(), ()> {
        let Ok(event) = bincode::deserialize(message.into_data().as_slice()) else {
            return Err(());
        };
        match event {
            UserEvent::DirectionChange(d) => self.direction_change_event(d, &hub),
            UserEvent::Yaw(yaw) => self.yaw_change_event(yaw, &hub),
            UserEvent::SetShooting(shooting) => self.shooting = shooting,
            UserEvent::LevelUpgrade(stat) => self.increment_level(stat)
        };
        Ok(())
    }

    pub fn change_direction(&mut self, direction: DirectionChange) {
        let velocity = direction.to_velocity();
        let acceleration_x = velocity.x / 10.;
        let acceleration_y = velocity.y / 10.;
        self.acceleration = Coordinates {
            x: if acceleration_x != 0. {acceleration_x} else {-self.max_velocity.x / 10.},
            y: if acceleration_y != 0. {acceleration_y} else {-self.max_velocity.y / 10.}
        };
        self.max_velocity = velocity;
    }

    fn yaw_change_event(&mut self, yaw: i32, hub: &Arc<Hub>) {
        if yaw == self.yaw {
            return;
        }
        self.yaw = yaw;
        hub.dispatch_event(EventData {
            event: Event::EntityUpdate,
            data: &self
        });
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
    pub author: Uuid
}