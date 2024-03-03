use std::{array, collections::HashSet, sync::{atomic::{AtomicI32, Ordering}, Arc}, time::Duration};
use dashmap::DashMap;
use futures_util::StreamExt;
use log::warn;
use rand::Rng;
use tokio::{net::TcpStream, sync::{broadcast, RwLock}, time};
use tokio_tungstenite::WebSocketStream;
use tungstenite::Message;
use crate::{events::ServerEvent, players::{forward_messages_from_channel, listen_for_messages, Coordinates, Entity, EntityType, Player}, Config};

pub static ID_COUNTER: AtomicI32 = AtomicI32::new(0);

pub type Id = i32;

pub struct HubManager {
    hubs: DashMap<Id, Arc<Hub>>,
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
        self.hubs.insert(ID_COUNTER.fetch_add(1, Ordering::SeqCst), new_hub.clone());
        new_hub
    }

    pub async fn create_client(&self, stream: WebSocketStream<TcpStream>) {
        let hub = self.find_hub().unwrap_or_else(|| self.create_hub());
        hub.spawn_player(stream).await;
    }
}

pub struct Hub {
    clients: DashMap<Id, broadcast::Sender<Message>>,
    entities: DashMap<Id, Arc<RwLock<Entity>>>,
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
    
    async fn update_entity(&self, entity: &Arc<RwLock<Entity>>, tiles: &mut PlayerPositions, tick: &u32, events: &mut Vec<ServerEvent>) {
        let mut entity_data = entity.write().await;
        let old_coords = entity_data.coordinates.clone();
        entity_data.move_once();
        if tiles.add(&entity_data.coordinates, entity_data.id) {
            tiles.remove(entity_data.id, &old_coords);
        }
        entity_data.tick(tick, events);
        events.push(ServerEvent::Position { user: entity_data.id, coordinates: entity_data.coordinates.clone() });
    } 

    async fn update_all_entities(&self, tiles: &mut PlayerPositions, tick: &u32, events: &mut Vec<ServerEvent>) {
        for entity in self.entities.iter() {
            self.update_entity(entity.value(), tiles, tick, events).await;
        }
    } 

    async fn game_update_loop(&self) {
        let mut tiles = PlayerPositions::new(self.config.map_size / 10.);
        let mut interval = time::interval(Duration::from_millis(self.config.update_delay_ms));
        let mut tick = 0u32;
        loop {
            interval.tick().await;
            let mut events = Vec::new();
            self.update_all_entities(&mut tiles, &tick, &mut events).await;
            self.dispatch_events(events);
            tick += 1;
        }
    }

    fn random_coordinate(&self) -> f64 {
        rand::thread_rng().gen_range(0..self.config.map_size as i32) as f64
    }

    pub fn dispatch_event(&self, event: ServerEvent) {
        self.dispatch_events(vec![event]);
    }

    pub fn dispatch_events(&self, events: Vec<ServerEvent>) {
        let data = bincode::serialize(&events).unwrap();
        for client in self.clients.iter() {
            if let Err(_) = client.send(Message::Binary(data.clone())) {
                warn!("Sent message to channel nobody was listening to");
            }
        }
    }

    pub fn kick_player(&self, id: Id) {
        self.clients.remove(&id);
        self.remove_entity(id);
    }

    pub fn remove_entity(&self, id: Id) {
        if self.entities.remove(&id).is_none() {
            warn!("Tried removing an entity that does not exist: {}", id);
            return;
        }
        self.dispatch_event(ServerEvent::EntityDelete(id));
    }

    pub async fn spawn_entity(&self, entity: Entity) -> Arc<RwLock<Entity>> {
        let id = entity.id.clone();
        let entity_arc = Arc::new(RwLock::new(entity));
        let ent = entity_arc.clone();
        let data = ent.read().await;
        self.dispatch_event(ServerEvent::EntityCreate { id, tank: data.tank.id, position: data.coordinates.clone() });
        self.entities.insert(id, entity_arc.clone());
        entity_arc
    }

    pub async fn spawn_player(self: &Arc<Self>, stream: WebSocketStream<TcpStream>) {
        let (mut ws_sender, ws_receiver) = stream.split();
        let (sender, mut receiver) = broadcast::channel::<Message>(16);
        let coords = Coordinates {
            x: self.random_coordinate(),
            y: self.random_coordinate()
        };
        let id = ID_COUNTER.fetch_add(1, Ordering::SeqCst);
        let entity_data = Entity::new(coords, id, &self.config, EntityType::Player(Player { points: 0, score: 0 }));
        let entity = self.spawn_entity(entity_data).await;

        tokio::spawn(listen_for_messages(entity.clone(), ws_receiver, self.clone()));
        
        forward_messages_from_channel(
            &mut ws_sender, 
            &mut receiver
        ).await;
        self.clients.insert(id, sender);
    }
}

type Tile = Option<HashSet<Id>>;

struct PlayerPositions {
    tiles: [[Tile; 10]; 10],
    scale: f64
}

impl PlayerPositions {

    fn new(scale: f64) -> Self {
        Self {
            tiles: array::from_fn(|_| array::from_fn(|_| None)),
            scale
        }
    }

    fn get(&mut self, coords: &Coordinates) -> &mut Tile {
        &mut self.tiles[coords.y as usize / self.scale as usize][coords.x as usize / self.scale as usize]
    }

    fn add(&mut self, coords: &Coordinates, id: Id) -> bool {
        let tile = self.get(coords);
        if let Some(entities) = tile {
            entities.insert(id)
        } else {
            let mut values = HashSet::new();
            values.insert(id);
            *tile = Some(values);
            true
        }
    }

    fn remove(&mut self, id: Id, coords: &Coordinates) {
        let tile = self.get(coords);
        if let Some(entities) = tile {
            entities.remove(&id);
            if entities.is_empty() {
                *tile = None;
            }
        }
    }
}