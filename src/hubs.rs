use std::{array, collections::HashMap, sync::Arc, time::Duration};
use dashmap::DashMap;
use futures_util::StreamExt;
use log::warn;
use rand::Rng;
use serde::Serialize;
use tokio::{net::TcpStream, sync::{broadcast, RwLock}, time};
use tokio_tungstenite::WebSocketStream;
use tungstenite::Message;
use uuid::Uuid;
use crate::{events::{Event, EventData, Identity}, players::{forward_messages_from_channel, listen_for_messages, Coordinates, Entity, EntityType, Player}, Config};

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
        let min_hub = self.hubs.iter().min_by_key(|h| h.clients.len()).unwrap();
        if min_hub.clients.len() > min_hub.config.max_player_count as usize {
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
    clients: DashMap<Uuid, broadcast::Sender<Message>>,
    entities: DashMap<Uuid, Arc<RwLock<Entity>>>,
    config: Config
}

impl Hub {

    pub fn new(config: &Config) -> Hub {
         Hub {
            clients: DashMap::new(),
            entities: DashMap::new(),
            config: config.clone()
        }
    }

    fn start(self: Arc<Self>) {
        tokio::spawn(async move {
            self.game_update_loop().await;
        });
    }
    
    async fn move_entity(&self, entity: &Arc<RwLock<Entity>>, tiles: &mut PlayerPositions) {
        let mut entity_data = entity.write().await;
        let old_coords = entity_data.coordinates.clone();
        entity_data.move_once();
        if tiles.add(entity, &entity_data.coordinates, entity_data.id) {
            tiles.remove(entity_data.id, &old_coords);
        }
    } 

    async fn update_all_entities(&self, tiles: &mut PlayerPositions) {
        for entity in self.entities.iter() {
            self.move_entity(entity.value(), tiles).await;
        }
    } 

    async fn game_update_loop(&self) {
        let mut tiles = PlayerPositions::new(self.config.map_size / 10.);
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
        let data = event.to_json();
        for client in self.clients.iter() {
            if let Err(_) = client.send(Message::Binary(data.clone())) {
                warn!("Sent message to channel nobody was listening to");
            }
        }
    }

    pub fn kick_player(&self, id: Uuid) {
        self.clients.remove(&id);
        self.remove_entity(id);
    }

    pub fn remove_entity(&self, id: Uuid) {
        if let Some(_) = self.entities.remove(&id) {
            self.dispatch_event(&EventData {
                event: Event::EntityDelete,
                data: Identity {
                    id
                }
            });
        } else {
            warn!("Tried removing an entity that does not exist: {}", id);
        }
    }

    pub async fn spawn_entity(&self, entity: Entity) -> Arc<RwLock<Entity>> {
        let entity_arc = Arc::new(RwLock::new(entity.clone()));
        self.entities.insert(entity.id, entity_arc.clone());
        self.dispatch_event(&EventData {
            event: Event::EntityCreate,
            data: entity
        });
        return entity_arc;
    }

    pub async fn spawn_player(self: &Arc<Self>, stream: WebSocketStream<TcpStream>) {
        let (mut ws_sender, ws_receiver) = stream.split();
        let (sender, mut receiver) = broadcast::channel::<Message>(16);
        let coords = Coordinates {
            x: self.random_coordinate(),
            y: self.random_coordinate()
        };
        let id = Uuid::new_v4();
        let entity = self.spawn_entity(
            Entity::new(coords, id, &self.config, EntityType::Player(Player {points: 0, score: 0}))
        ).await;

        tokio::spawn(listen_for_messages(entity.clone(), ws_receiver, self.clone()));
        self.clients.insert(id, sender);
        forward_messages_from_channel(
            &mut ws_sender, 
            &mut receiver
        ).await;
    }
}

type Tile = Option<HashMap<Uuid, Arc<RwLock<Entity>>>>;

struct PlayerPositions {
    tiles: [[Tile; 10]; 10],
    scale: f64
}

impl PlayerPositions {

    fn new(scale: f64) -> Self {
        Self {
            tiles: array::from_fn(|_|  array::from_fn(|_| None)),
            scale
        }
    }

    fn get(&mut self, coords: &Coordinates) -> &mut Tile {
        &mut self.tiles[coords.y as usize / self.scale as usize][coords.x as usize / self.scale as usize]
    }

    fn add(&mut self, entity: &Arc<RwLock<Entity>>, coords: &Coordinates, id: Uuid) -> bool {
        let tile = self.get(coords);
        if let Some(entities) = tile {
            entities.insert(id, entity.clone()).is_none()
        } else {
            let mut values = HashMap::new();
            values.insert(id, entity.clone());
            *tile = Some(values);
            true
        }
    }

    fn remove(&mut self, id: Uuid, coords: &Coordinates) {
        let tile = self.get(coords);
        if let Some(entities) = tile {
            entities.remove(&id);
            if entities.is_empty() {
                *tile = None;
            }
        }
    }
}