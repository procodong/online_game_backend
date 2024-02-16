use std::{array, sync::Arc, time::Duration};
use dashmap::{DashMap, DashSet};
use futures_util::StreamExt;
use log::warn;
use rand::Rng;
use serde::Serialize;
use tokio::{net::TcpStream, sync::{broadcast, RwLock}, time};
use tokio_tungstenite::WebSocketStream;
use uuid::Uuid;
use crate::{events::{Event, EventData}, players::{forward_messages_from_channel, listen_for_messages, Coordinates, Entity, Player}, Config};

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
        if min_hub.players.len() > self.config.max_player_count as usize {
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
    config: Config,
    tiles: EntityTree
}

impl Hub {

    pub fn new(config: &Config) -> Hub {
         Hub {
            players: DashMap::new(),
            entities: DashMap::new(),
            config: config.clone(),
            tiles: EntityTree::Single(DashSet::new())
        }
    }

    fn start(self: Arc<Self>) {
        tokio::spawn(async move {
            self.game_update_loop().await;
        });
    }
    
    async fn move_entity(&self, entity: &Arc<RwLock<Entity>>) {
        let mut entity_data = entity.write().await;
        let old_tile = self.tiles.find_entity(&entity_data, self.config.map_size);
        entity_data.move_once();
        let tile = self.tiles.find_entity(&entity_data, self.config.map_size);
        if !tile.contains(&entity_data.id) {
            old_tile.remove(&entity_data.id);
            tile.insert(entity_data.id);
        }
    } 

    async fn update_all_entities(&self) {
        for entity in self.entities.iter() {
            self.move_entity(entity.value()).await;
        }
    } 

    async fn game_update_loop(&self) {
        let mut interval = time::interval(Duration::from_millis(self.config.update_delay_ms));
        loop {
            interval.tick().await;
            self.update_all_entities().await;
        }
    }

    fn random_coordinate(&self) -> f64 {
        rand::thread_rng().gen_range(0..self.config.map_size as i32) as f64
    }

    pub fn dispatch_event<T: Serialize + ?Sized>(&self, event: &EventData<T>) {
        let data = event.to_json().unwrap();
        self.send(&data);
    }

    pub fn send(&self, data: &Vec<u8>) {
        for player in self.players.iter() {
            if let Err(_) = player.messages.send(data.clone()) {
                warn!("Sent message channel nobody was listening to");
            }
        }
    }


    pub async fn spawn_entity(&self, entity: &Entity) -> Arc<RwLock<Entity>> {
        let event = EventData {
            event: Event::EntityCreate,
            data: entity.clone()
        };
        let entity_arc = Arc::new(RwLock::new(entity.clone()));
        self.entities.insert(entity.id, entity_arc.clone());
        self.dispatch_event(&event);
        return entity_arc;
    }

    pub async fn spawn_player(self: &Arc<Self>, stream: WebSocketStream<TcpStream>) {
        let (mut ws_sender, ws_receiver) = stream.split();

        let (sender, mut receiver) = broadcast::channel::<Vec<u8>>(16);
        let coords = Coordinates {
            x: self.random_coordinate(),
            y: self.random_coordinate()
        };
        let id = Uuid::new_v4();
        let entity = self.spawn_entity(&Entity::new(coords, id)).await;

        tokio::spawn(listen_for_messages(entity.clone(), ws_receiver, self.clone()));

        let player = Player {
            messages: sender
        };
        self.players.insert(id, player);
        forward_messages_from_channel(
            &mut ws_sender, 
            &mut receiver).await;
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

    pub fn split(self, offset_x: f64, offset_y: f64, size: f64, entities: &DashMap<Uuid, Entity>) -> EntityTree {
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
            if coords.x < offset_x || coords.y < offset_y || coords.x > offset_x + size || coords.y > offset_y + size {
                println!("entity found at wrong tile");
                continue;
            }
            let (x, y) = get_tile_index(coords.x - offset_x, coords.y - offset_y, size);
            if let EntityTree::Single(v) = &splitted_values[y][x] {
                v.insert(*id);
            }
        }
        EntityTree::Quad(Box::new(splitted_values))
    }

    fn scan(&self, tile_size: f64, relative_x: f64, relative_y: f64) -> &DashSet<Uuid> {
        if let EntityTree::Single(v) = self {
            return v;
        }
        let EntityTree::Quad(tree) = self else {panic!()};
        let (x_tile_index, y_tile_index) = get_tile_index(relative_x, relative_y, tile_size);
        let value = &tree[y_tile_index][x_tile_index];
        return value.scan(
            tile_size / 2., 
            relative_x - tile_size * x_tile_index as f64, 
            relative_y - tile_size * y_tile_index as f64
        );
    }

    fn find_entity(&self, entity: &Entity, map_size: f64) -> &DashSet<Uuid> {
        self.scan(map_size, entity.coordinates.x, entity.coordinates.y)
    }
}
