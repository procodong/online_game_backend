use serde::{Deserialize, Serialize};
use serde;

#[derive(Deserialize, Serialize)]
#[serde()]
pub struct EventData<T: ?Sized + Serialize> {
    pub event: Event,
    pub data: T
}
impl<T: ?Sized + Serialize> EventData<T> {

    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string(self)
    }
}

#[derive(Deserialize, Serialize)]
pub enum Event {
    MotionUpdate = 0,
    EntityCreate = 1
}
