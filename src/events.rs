use serde::{Deserialize, Serialize};
use serde;
use uuid::Uuid;

#[derive(Deserialize, Serialize)]
pub struct EventData<T: ?Sized + Serialize> {
    pub event: Event,
    pub data: T
}
impl<T: ?Sized + Serialize> EventData<T> {

    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap()
    }
}

#[derive(Serialize, Deserialize)]
pub enum Event {
    EntityCreate = 0,
    EntityDelete = 1,
    DirectionChange = 2,
    YawChange = 3,
    MotionUpdate = 4,
    ToggleShooting= 5
}

#[derive(Serialize, Deserialize)]
pub struct DirectionChange {
    pub up: bool,
    pub left: bool,
    pub down: bool,
    pub right: bool
}

#[derive(Serialize, Deserialize)]
pub struct YawChange {
    pub yaw: i32
}

#[derive(Serialize, Deserialize)]
pub struct Identity {
    pub id: Uuid
}