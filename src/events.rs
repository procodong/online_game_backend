use serde::{Deserialize, Serialize};
use serde;
use uuid::Uuid;

use crate::players::{Coordinates, Stat};

#[derive(Deserialize, Serialize)]
pub struct EventData<T: ?Sized + Serialize> {
    pub event: Event,
    pub data: T
}


#[derive(Serialize, Deserialize)]
pub enum Event {
    EntityDelete = 0,
    DirectionChange = 1,
    YawChange = 2,
    EntityUpdate = 3,
    ToggleShooting = 4,
    LevelUpgrade = 5
}

#[derive(Deserialize)]
pub enum UserEvent {
    SetShooting(bool),
    Yaw(i32),
    LevelUpgrade(Stat),
    DirectionChange(DirectionChange)
}

#[derive(Serialize)]
pub enum ServerEvent {
    EntityDelete(Uuid),
    EntityCreate {id: Uuid, tank: i32, position: Coordinates},
    Yaw {user: Uuid, yaw: i32},
    Position {user: Uuid, coordinates: Coordinates},
    TankUpgrade {user: Uuid, tank: i32},
}

#[derive(Deserialize)]
pub struct DirectionChange {
    pub up: bool,
    pub left: bool,
    pub down: bool,
    pub right: bool
}

impl DirectionChange {
    pub fn to_velocity(&self) -> Coordinates {
        Coordinates {
            x: self.right as i32 as f64 - self.left as i32 as f64,
            y: self.down as i32 as f64 - self.up as i32 as f64
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct Identity {
    pub id: Uuid
}