

/* 
#[cfg(test)]
mod tests {
    //use crate::hubs::Hub;
    use uuid::Uuid;
    use crate::{events::DirectionChange, players::{Coordinates, Entity}};
    #[test]
    fn entity_movement() {
        //let test_hub = Hub::new(&crate::Config { max_player_count: 1, map_size: 100., update_delay_ms: 250 });
        let mut entity = Entity::new(Coordinates {x: 0., y: 0.}, Uuid::new_v4());
        for _ in 0..10 {
            entity.change_direction(DirectionChange {up: false, down: true, left: false, right: true});
            println!("{:?}", entity)
        }
    }
}
*/