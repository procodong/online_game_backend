use serde::{Deserialize, Serialize};
use serde;

#[derive(Deserialize, Serialize)]
#[serde()]
pub struct EventData<T: ?Sized + Serialize> {
    pub event: Event,
    pub data: T
}
impl<T: ?Sized + Serialize> EventData<T> {

    pub fn to_json(&self) -> serde_json::Result<Vec<u8>> {
        serde_json::to_vec(self)
    }
}

#[derive(Serialize, Deserialize)]
pub enum Event {
    EntityCreate = 0,
    EntityDelete = 1,
    DirectionChange = 2,
    YawChange = 3,
    MotionUpdate = 4
}

#[derive(Serialize, Deserialize)]
pub struct DirectionChange {
    pub direction: i32
}

#[derive(Serialize, Deserialize)]
pub struct YawChange {
    pub yaw: i32
}
