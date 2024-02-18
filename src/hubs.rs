use std::{array, sync::Arc, time::Duration};
use dashmap::{DashMap, DashSet};
use futures_util::StreamExt;
use log::warn;
use rand::Rng;
use serde::Serialize;
use tokio::{net::TcpStream, sync::{broadcast, RwLock}, time};
use tokio_tungstenite::WebSocketStream;
use uuid::Uuid;
use crate::{events::{Event, EventData, Identifiable}, players::{forward_messages_from_channel, listen_for_messages, Coordinates, Entity, Player}, Config};

pub struct HubManager {
    hubs: DashMap<Uuid, Arc<Hub>>,
    config: Config
}
impl HubManager {

    pub async fn new() -> HubManager {
        HubManager { hubs: DashMap::new(), config: Config::get().await }
    }

    fn find_hub(&self) -> Option<Arc<Hub>> {
        if self.hubs.len() == 0 {
            return None;
        }
        let min_hub = self.hubs.iter().min_by_key(|h| h.players.len()).unwrap();
        if min_hub.players.len() > min_hub.config.max_player_count as usize {
            return None;
        }
        Some(min_hub.clone())
    }

    fn create_hub(&self) -> Arc<Hub> {
        let new_hub = Arc::new(Hub::new(&self.config));
        new_hub.clone().start();
        self.hubs.insert(Uuid::new_v4(), new_hub.clone());
        new_hub
    }

    pub async fn create_client(&self, stream: WebSocketStream<TcpStream>) {
        let hub = self.find_hub().unwrap_or_else(|| self.create_hub());
        hub.spawn_player(stream).await;
    }
}

pub struct Hub {
    players: DashMap<Uuid, Player>,
    entities: DashMap<Uuid, Arc<RwLock<Entity>>>,
    config: Config
}

impl Hub {

    pub fn new(config: &Config) -> Hub {
         Hub {
            players: DashMap::new(),
            entities: DashMap::new(),
            config: config.clone()
        }
    }

    fn start(self: Arc<Self>) {
        tokio::spawn(async move {
            self.game_update_loop().await;
        });
    }
    
    async fn move_entity(&self, entity: &Arc<RwLock<Entity>>, tiles: &mut EntityTree) {
        let mut entity_data = entity.write().await;
      //  let old_coords = entity_data.coordinates.clone();
        entity_data.move_once();
       // if tiles.add(entity_data.id, &entity_data.coordinates, &self.config, self.entities) {

       // }
    } 

    async fn update_all_entities(&self, tiles: &mut EntityTree) {
        for entity in self.entities.iter() {
            self.move_entity(entity.value(), tiles).await;
        }
    } 

    async fn game_update_loop(&self) {
        let mut tiles = EntityTree::Single(DashSet::new());
        let mut interval = time::interval(Duration::from_millis(self.config.update_delay_ms));
        loop {
            interval.tick().await;
            self.update_all_entities(&mut tiles).await;
        }
    }

    fn random_coordinate(&self) -> f64 {
        rand::thread_rng().gen_range(0..self.config.map_size as i32) as f64
    }

    pub fn dispatch_event<T: Serialize + ?Sized>(&self, event: &EventData<T>) {
        let data = event.to_json().unwrap();
        for player in self.players.iter() {
            if let Err(_) = player.messages.send(data.clone()) {
                warn!("Sent message to channel nobody was listening to");
            }
        }
    }

    pub fn kick_player(&self, id: Uuid) {
        if let Some((_, player)) = self.players.remove(&id) {
            if let Err(e) = player.closer.send(()) {
                warn!("Error kicking player: {:?}", e);
            }
        }
        self.remove_entity(id);
    }

    pub fn remove_entity(&self, id: Uuid) {
        if let Some(_) = self.entities.remove(&id) {
            self.dispatch_event(&EventData {
                event: Event::EntityDelete,
                data: Identifiable {
                    id
                }
            });
        } else {
            warn!("Tried removing an entity that does not exist: {}", id);
        }
    }

    pub async fn spawn_entity(&self, entity: &Entity) -> Arc<RwLock<Entity>> {
        let entity_arc = Arc::new(RwLock::new(entity.clone()));
        self.entities.insert(entity.id, entity_arc.clone());
        self.dispatch_event(&EventData {
            event: Event::EntityCreate,
            data: entity.clone()
        });
        return entity_arc;
    }

    pub async fn spawn_player(self: &Arc<Self>, stream: WebSocketStream<TcpStream>) {
        let (mut ws_sender, ws_receiver) = stream.split();

        let (sender, mut receiver) = broadcast::channel::<Vec<u8>>(16);
        let (closer, mut close_messages) = broadcast::channel::<()>(16);
        let coords = Coordinates {
            x: self.random_coordinate(),
            y: self.random_coordinate()
        };
        let id = Uuid::new_v4();
        let entity = self.spawn_entity(&Entity::new(coords, id)).await;

        tokio::spawn(listen_for_messages(entity.clone(), ws_receiver, self.clone()));

        let player = Player {
            messages: sender,
            closer
        };
        self.players.insert(id, player);
        forward_messages_from_channel(
            &mut ws_sender, 
            &mut receiver,
            &mut close_messages
        ).await;
    }
}

pub enum EntityTree {
    Single(DashSet<Uuid>),
    Quad(Box<[[EntityTree; 2]; 2]>)
}


fn get_tile_index(relative_x: f64, relative_y: f64, tile_size: f64) -> (usize, usize) {
    (
        (relative_x / tile_size).round() as usize, 
        (relative_y / tile_size).round() as usize
    )
}

impl EntityTree {

    pub fn split(self, offset: Coordinates, size: f64, entities: &DashMap<Uuid, Entity>) -> EntityTree {
        if let EntityTree::Quad(_) = self {
            return self;
        }
        let EntityTree::Single(values) = self else {panic!()};
        let splitted_values: [[EntityTree; 2]; 2] = array::from_fn(|_|array::from_fn(|_| EntityTree::Single(DashSet::new())));
        for id in values.iter() {
            let entity = entities.get(&id);
            if entity.is_none() {
                continue;
            }
            let coords = &entity.unwrap().coordinates;
            if coords.x < offset.x || coords.y < offset.y || coords.x > offset.x + size || coords.y > offset.y + size {
                println!("entity found at wrong tile");
                continue;
            }
            let (x, y) = get_tile_index(coords.x - offset.x, coords.y - offset.y, size);
            if let EntityTree::Single(v) = &splitted_values[y][x] {
                v.insert(*id);
            }
        }
        EntityTree::Quad(Box::new(splitted_values))
    }

    pub fn add(&mut self, id: Uuid, coords: &Coordinates, config: &Config, other_entities: &DashMap<Uuid, Entity>) -> bool {
        let (tile, offset, size) = self.find_entity(coords, config);
        if let EntityTree::Single(entities) = tile {
            let result = entities.insert(id);
            if entities.len() > config.max_tile_player_count {
              //  *tile = tile.split(offset, size, other_entities);
            }
            return result;
        }
        false
    }

    pub fn remove(&mut self, id: Uuid, coords: &Coordinates, config: &Config) {
        let (tile, offset, size) = self.find_entity(coords, config);
        if let EntityTree::Single(entities) = tile {
            entities.remove(&id);
        }
    }

    fn scan(&mut self, tile_size: f64, relative_x: f64, relative_y: f64, offset_x: f64, offset_y: f64) -> (&mut EntityTree, Coordinates, f64) {
        if let EntityTree::Single(_) = self {
            return (self, Coordinates {x: offset_x, y: offset_y}, tile_size);
        }
        let EntityTree::Quad(tree) = self else {panic!()};
        let (x_tile_index, y_tile_index) = get_tile_index(relative_x, relative_y, tile_size);
        let value = &mut tree[y_tile_index][x_tile_index];
        let x_quad_pos = tile_size * x_tile_index as f64;
        let y_quad_pos = tile_size * y_tile_index as f64;
        value.scan(
            tile_size / 2., 
            relative_x - x_quad_pos, 
            relative_y - y_quad_pos,
            offset_x + x_quad_pos,
            offset_y + y_quad_pos
        )
    }

    fn find_entity(&mut self, coords: &Coordinates, config: &Config) -> (&mut EntityTree, Coordinates, f64) {
        self.scan(config.map_size, coords.x, coords.y, 0., 0.)
    }
}
