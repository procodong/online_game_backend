use std::{array, time::Duration};
use futures_util::SinkExt;
use indexmap::{IndexMap, IndexSet};
use rand::Rng;
use tokio::{net::TcpStream, sync::{broadcast, mpsc}, time};
use tokio_tungstenite::WebSocketStream;
use crate::{events::{ServerEvent, UserInit, UserMessage}, players::{handle_client_connection, Entity, EntityType, Player, Vec2}, Config};


pub type Id = u32;

struct IdCounter(Id);

impl IdCounter {
    fn next(&mut self) -> Id {
        self.0 += 1;
        self.0
    }
}

pub struct HubManager {
    hubs: IndexMap<Id, HubPlayers>,
    config: Config,
    ids: IdCounter
}

impl HubManager {

    pub async fn new() -> HubManager {
        HubManager { hubs: IndexMap::new(), config: Config::get().await, ids: IdCounter(0) }
    }

    async fn create_hub(&mut self, stream: WebSocketStream<TcpStream>) {
        let mut new_hub = Hub::new(self.config.clone());
        let (user_adder, user_receiver) = mpsc::channel(32);
        let _ = user_adder.send(stream).await;
        self.hubs.insert(self.ids.next(), HubPlayers { adder: user_adder, player_count: 0 });
        tokio::spawn(async move {
            new_hub.game_update_loop(user_receiver).await;
        });
    }

    pub async fn create_client(&mut self, stream: WebSocketStream<TcpStream>) {
        let found_hub = self.hubs.values_mut().min_by_key(|h| h.player_count);
        match found_hub {
            Some(hub) if hub.player_count < self.config.max_player_count => {
                if hub.adder.send(stream).await.is_ok() {
                    hub.player_count += 1;
                } 
            },
            _ => self.create_hub(stream).await
        };
    }
}

struct HubPlayers {
    adder: mpsc::Sender<WebSocketStream<TcpStream>>,
    player_count: i32
}

struct Hub {
    entities: IndexMap<Id, Entity>,
    config: Config,
    queued_events: Vec<ServerEvent>,
    ids: IdCounter,
    tiles: PlayerPositions<100>
}

impl Hub {

    fn new(config: Config) -> Hub {
         Hub {
            entities: IndexMap::new(),
            tiles: PlayerPositions::new(config.map_size),
            config,
            queued_events: Vec::new(),
            ids: IdCounter(0),
        }
    }

    fn update_entity(&mut self, entity: &mut Entity, id: Id, tick: u32) {
        let old_coords = entity.coordinates;

        entity.update_movement(self.config.map_size);

        if self.tiles.add(&entity.coordinates, id) {
            self.tiles.remove(&old_coords, id);
        }
        if !matches!(entity.inner, EntityType::Prop) || entity.coordinates != old_coords {
            self.queued_events.push(ServerEvent::Position { user: id, coordinates: entity.coordinates, velocity: entity.velocity, yaw: entity.yaw });
        }
        if entity.shooting {
            for cannon in entity.active_cannons(tick) {
                let bullet = entity.create_bullet(cannon, id);
                self.spawn_entity(bullet);
            }
        }
    }

    fn entity_collisions(&mut self, entities: &IndexMap<Id, Entity>) -> Vec<(Id, f32)> {
        let mut hits = Vec::new();
        for (id, entity) in entities.iter() {
            let Some(tile) = self.tiles.get_mut(&entity.coordinates) else {
                continue;
            };
            for other_id in tile.iter() {
                if id == other_id {
                    continue;
                }
                let Some(other_entity) = entities.get(other_id) else {
                    continue;
                };
                if entity.distance_from(other_entity) < entity.tank.size + other_entity.tank.size {
                    hits.push((*other_id, entity.stat(crate::players::Stat::BodyDamage)))
                }
            }
        }
        hits
    }
 
    fn update_entities(&mut self, tick: u32) {
        let mut entities = std::mem::take(&mut self.entities);

        for (id, entity) in entities.iter_mut() {
            self.update_entity(entity, *id, tick);
        }
        let collisions = self.entity_collisions(&entities);

        let created_bullets = std::mem::replace(&mut self.entities, entities);

        self.entities.extend(created_bullets);

        for (id, damage) in collisions {
            let Some(entity) = self.entities.get_mut(&id) else {
                continue;
            };
            if !entity.damage(damage) {
                if matches!(entity.inner, EntityType::Prop)  {
                    let tank = entity.tank.clone();
                    self.spawn_entity(Entity::new(Vec2 { x: self.random_coordinate(), y: self.random_coordinate() }, tank, EntityType::Prop));
                }
                self.remove_entity(id);
            }
        }
    }

    fn random_coordinate(&self) -> f64 {
        let size = self.config.map_size as i32;
        rand::thread_rng().gen_range(-size..size) as f64
    }

    async fn game_update_loop(&mut self, mut user_adder: mpsc::Receiver<WebSocketStream<TcpStream>>) {
        let mut interval = time::interval(Duration::from_millis(self.config.update_delay_ms));
        let mut tick = 0;
        let (update_sender, mut received_updates) = mpsc::channel(128);
        let (event_sender, _) = broadcast::channel(128);
        loop {
            tokio::select! {
                biased;
                _ = interval.tick() => {
                    self.update_entities(tick);
                    let data = bincode::serialize(&self.queued_events).unwrap();
                    let _ = event_sender.send(data);
                    self.queued_events.clear();
                    tick += 1;
                },
                message = user_adder.recv() => {
                    match message {
                        Some(stream) => self.spawn_player(stream, update_sender.clone(), event_sender.subscribe()),
                        _ => break
                    };
                },
                Some(message) = received_updates.recv() => {
                    match message {
                        UserMessage::Event { user, event } => {
                            if let Some(user) = self.entities.get_mut(&user) {
                                user.handle_event(event);
                            }
                        },
                        UserMessage::GoingAway(id) => {
                            self.remove_entity(id);
                        }
                    }
                }
            }
        }
    }

    fn remove_entity(&mut self, id: Id) -> Option<Entity> {
        let entity = self.entities.swap_remove(&id)?;
        self.tiles.remove(&entity.coordinates, id);
        self.queued_events.push(ServerEvent::EntityDelete { id });
        Some(entity)
    }

    fn spawn_entity(&mut self, entity: Entity) -> Id {
        let id = self.ids.next();
        self.tiles.add(&entity.coordinates, id);
        self.queued_events.push(ServerEvent::EntityCreate { id, tank: entity.tank.id, position: entity.coordinates });
        self.entities.insert(id, entity);
        id
    }

    fn spawn_player(&mut self, mut stream: WebSocketStream<TcpStream>, update_sender: mpsc::Sender<UserMessage>, events: broadcast::Receiver<Vec<u8>>) {
        let entity = Entity::new(Vec2::default(), self.config.tanks[0].clone(), EntityType::Player(Player { points: 0, score: 0 }));
        let id = self.spawn_entity(entity);

        let init = bincode::serialize(&UserInit { config: &self.config, you: id }).unwrap();
        let _ = stream.send(tungstenite::Message::Binary(init));

        tokio::spawn(handle_client_connection(stream, events, update_sender, id));
    }
}

type Tile = IndexSet<Id>;

struct PlayerPositions<const I: usize> {
    tiles: [Tile; I],
    scale: usize
}

impl <const I: usize> PlayerPositions<I> {

    fn new(size: f64) -> Self {
        Self {
            tiles: array::from_fn(|_| IndexSet::new()),
            scale: size as usize / 10
        }
    }

    fn index(&self, pos: &Vec2) -> usize {
        let x = pos.x.abs() as usize / self.scale;
        let y = pos.y.abs() as usize / self.scale;
        I / 10 * y + x
    }

    fn get_mut(&mut self, pos: &Vec2) -> Option<&mut Tile> {
        let index = self.index(pos);
        self.tiles.get_mut(index)
    }

    fn add(&mut self, coords: &Vec2, id: Id) -> bool {
        if let Some(tile) = self.get_mut(coords) {
            tile.insert(id)
        } else {false}
    }

    fn remove(&mut self, coords: &Vec2, id: Id) {
        if let Some(tile) = self.get_mut(coords) {
            tile.swap_remove(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::players::Vec2;

    use super::PlayerPositions;

    #[test]
    fn player_positions() {
        let mut positions: PlayerPositions<100> = PlayerPositions::new(100.);

        let pos = Vec2 {x: 99., y: 99. };

        assert!(positions.add(&pos, 0));

        assert!(!positions.add(&pos, 0));

        let pos = Vec2 {x: 200., y: 200. };

        assert!(!positions.add(&pos, 0));

        let pos = Vec2 {x: 50., y: 70. };

        assert!(positions.add(&pos, 0));

        assert!(!positions.add(&pos, 0));
    }
}