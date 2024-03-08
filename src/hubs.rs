use std::{array, collections::{HashMap, HashSet}, sync::atomic::{AtomicI32, Ordering}, time::Duration};
use dashmap::DashMap;
use log::warn;
use rand::Rng;
use tokio::{net::TcpStream, sync::mpsc, time};
use tokio_tungstenite::WebSocketStream;
use tungstenite::Message;
use crate::{events::{ServerEvent, UserEventMessage}, players::{handle_client_connection, Coordinates, Entity, EntityType, Player}, Config};

pub static ID_COUNTER: AtomicI32 = AtomicI32::new(0);

pub type Id = i32;

pub struct HubManager {
    hubs: DashMap<Id, HubPlayers>,
    config: Config
}

impl HubManager {

    pub async fn new() -> HubManager {
        HubManager { hubs: DashMap::new(), config: Config::get().await }
    }

    fn find_hub(&self) -> Option<HubPlayers> {
        if self.hubs.len() == 0 {
            return None;
        }
        let min_hub = self.hubs.iter_mut().min_by_key(|h| h.player_count).unwrap();
        if min_hub.player_count > self.config.max_player_count {
            return None;
        }
        Some(min_hub.clone())
    }

    async fn create_hub(&self, stream: WebSocketStream<TcpStream>) {
        let mut new_hub = Hub::new(&self.config);
        let (user_adder, user_receiver) = mpsc::channel(1);
        let _ = user_adder.send(stream).await;
        self.hubs.insert(ID_COUNTER.fetch_add(1, Ordering::SeqCst), HubPlayers { adder: user_adder, player_count: 0 });
        tokio::spawn(async move {
            new_hub.game_update_loop(user_receiver).await;
        });
    }

    pub async fn create_client(&self, stream: WebSocketStream<TcpStream>) {
        match self.find_hub() {
            Some(hub) => {
                if hub.adder.send(stream).await.is_err() {
                    warn!("Tried to add a player to a hub that has ended");

                }
            },
            _ => self.create_hub(stream).await
        };
    }
}

#[derive(Debug, Clone)]
struct HubPlayers {
    adder: mpsc::Sender<WebSocketStream<TcpStream>>,
    player_count: i32
}

pub struct Hub {
    clients: HashMap<Id, mpsc::Sender<Message>>,
    entities: HashMap<Id, Entity>,
    config: Config,
    queued_events: Vec<ServerEvent>
}

impl Hub {

    pub fn new(config: &Config) -> Hub {
         Hub {
            clients: HashMap::new(),
            entities: HashMap::new(),
            config: config.clone(),
            queued_events: Vec::new()
        }
    }

    async fn update_all_entities(&mut self, tiles: &mut PlayerPositions, tick: &u32) {
        for (_, entity) in self.entities.iter_mut() {
            let old_coords = entity.coordinates.clone();
            entity.move_once();
            if tiles.add(&entity.coordinates, entity.id) {
                tiles.remove(entity.id, &old_coords);
            }
            entity.tick(tick, &mut self.queued_events);
            self.queued_events.push(ServerEvent::Position { user: entity.id, coordinates: entity.coordinates.clone() });
        }
    } 

    async fn game_update_loop(&mut self, mut user_adder: mpsc::Receiver<WebSocketStream<TcpStream>>) {
        let mut tiles = PlayerPositions::new(self.config.map_size / 10.);
        let mut interval = time::interval(Duration::from_millis(self.config.update_delay_ms));
        let mut tick = 0u32;
        let (update_sender, mut received_updates) = mpsc::channel::<UserEventMessage>(16);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    self.update_all_entities(&mut tiles, &tick).await;
                    self.dispatch_events().await;
                    self.queued_events.clear();
                    tick += 1;
                },
                Some(stream) = user_adder.recv() => {
                    self.spawn_player(stream, update_sender.clone());
                },
                Some(event) = received_updates.recv() => {
                    let user = self.entities.get_mut(&event.user);
                    if let Some(user) = user {
                        user.handle_event(event.event);
                    }
                }
            }
        }
    }

    fn random_coordinate(&self) -> f64 {
        rand::thread_rng().gen_range(0..self.config.map_size as i32) as f64
    }

    pub async fn dispatch_events(&self) {
        let data = bincode::serialize(&self.queued_events).unwrap();
        for (_, client) in self.clients.iter() {
            if let Err(_) = client.send(Message::Binary(data.clone())).await {
                warn!("Sent message to a client that isn't receiving messages");
            }
        }
    }

    // TODO: kick player after their connection ends
    pub fn kick_player(&mut self, id: Id) {
        self.clients.remove(&id);
        self.remove_entity(id);
    }

    pub fn remove_entity(&mut self, id: Id) {
        if self.entities.remove(&id).is_none() {
            warn!("Tried removing an entity that does not exist: {}", id);
            return;
        }
        self.queued_events.push(ServerEvent::EntityDelete(id));
    }

    pub fn spawn_entity(&mut self, entity: Entity) {
        self.queued_events.push(ServerEvent::EntityCreate { id: entity.id, tank: entity.tank.id, position: entity.coordinates.clone() });
        self.entities.insert(entity.id, entity);
    }

    pub fn spawn_player(&mut self, stream: WebSocketStream<TcpStream>, updates: mpsc::Sender<UserEventMessage>) {
        let (sender, receiver) = mpsc::channel::<Message>(1);
        let coords = Coordinates {
            x: self.random_coordinate(),
            y: self.random_coordinate()
        };
        let id = ID_COUNTER.fetch_add(1, Ordering::SeqCst);
        self.clients.insert(id, sender);
        let entity_data = Entity::new(coords, id, &self.config, EntityType::Player(Player { points: 0, score: 0 }));
        self.spawn_entity(entity_data);
        tokio::spawn(handle_client_connection(stream, receiver, updates, id));
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