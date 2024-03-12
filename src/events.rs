use serde::{Deserialize, Serialize};
use serde;
use crate::hubs::Id;
use crate::players::{Vector, Stat};


#[derive(Deserialize)]
#[serde(tag = "e")]
pub enum UserEvent {
    #[serde(rename = "0")]
    SetShooting { shooting: bool },
    #[serde(rename = "1")]
    Yaw { yaw: i32 },
    #[serde(rename = "2")]
    LevelUpgrade { stat: Stat },
    #[serde(rename = "3")]
    DirectionChange { direction: DirectionChange }
}

pub enum UserMessage {
    Event {
        event: UserEvent,
        user: Id
    },
    GoingAway(Id)
}
#[derive(Serialize)]
#[serde(tag = "e")]
pub enum ServerEvent {
    #[serde(rename = "0")]
    EntityDelete { id: Id },
    #[serde(rename = "1")]
    EntityCreate { id: Id, tank: i32, position: Vector },
    #[serde(rename = "2")]
    Position { user: Id, coordinates: Vector, yaw: i32, velocity: Vector },
    //TankUpgrade {user: Id, tank: i32},
}

#[derive(Deserialize, Clone)]
pub struct DirectionChange {
    up: bool,
    left: bool,
    down: bool,
    right: bool
}

impl DirectionChange {
    pub fn to_velocity(&self) -> Vector {
        Vector {
            x: self.right as i32 as f64 - self.left as i32 as f64,
            y: self.down as i32 as f64 - self.up as i32 as f64
        }
    }
}
