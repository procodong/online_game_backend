use std::{array, collections::{HashMap, HashSet}, sync::atomic::{AtomicI32, Ordering}, time::Duration};
use log::warn;
use tokio::{net::TcpStream, sync::mpsc, time};
use tokio_tungstenite::WebSocketStream;
use tungstenite::Message;
use crate::{events::{ServerEvent, UserMessage}, players::{handle_client_connection, Entity, EntityType, Player, Vector}, Config};


pub type Id = i32;

pub struct HubManager {
    hubs: HashMap<Id, HubPlayers>,
    config: Config,
    ids: AtomicI32
}

impl HubManager {

    pub async fn new() -> HubManager {
        HubManager { hubs: HashMap::new(), config: Config::get().await, ids: AtomicI32::new(0) }
    }

    async fn create_hub(&mut self, stream: WebSocketStream<TcpStream>) {
        let mut new_hub = Hub::new(&self.config);
        let (user_adder, user_receiver) = mpsc::channel(1);
        let _ = user_adder.send(stream).await;
        self.hubs.insert(self.ids.fetch_add(1, Ordering::SeqCst), HubPlayers { adder: user_adder, player_count: 0 });
        tokio::spawn(async move {
            new_hub.game_update_loop(user_receiver).await;
        });
    }

    pub async fn create_client(&mut self, stream: WebSocketStream<TcpStream>) {
        match self.hubs.values().min_by_key(|h| h.player_count) {
            Some(hub) if hub.player_count < self.config.max_player_count => {
                if hub.adder.send(stream).await.is_err() {
                    warn!("Tried to add a player to a hub that has ended");

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
    clients: HashMap<Id, mpsc::Sender<Message>>,
    entities: HashMap<Id, Entity>,
    config: Config,
    queued_events: Vec<ServerEvent>,
    ids: AtomicI32
}

impl Hub {

    fn new(config: &Config) -> Hub {
         Hub {
            clients: HashMap::new(),
            entities: HashMap::new(),
            config: config.clone(),
            queued_events: Vec::new(),
            ids: AtomicI32::new(0)
        }
    }

    fn next_id(&self) -> i32 {
        self.ids.fetch_add(1, Ordering::SeqCst)
    }
 
    async fn update_entities(&mut self, tiles: &mut PlayerPositions, tick: u32) {
        let mut created_bullets = Vec::new();
        for (_, entity) in self.entities.iter_mut() {
            let old_coords = entity.coordinates.clone();
            entity.move_once();
            if tiles.add(&entity.coordinates, entity.id) {
                tiles.remove(&old_coords, entity.id);
            }
            for cannon in entity.active_cannons(tick) {
                let entity = entity.create_bullet(cannon, self.ids.fetch_add(1, Ordering::SeqCst));
                created_bullets.push(entity);
            }
            entity.tick();
            self.queued_events.push(ServerEvent::Position { user: entity.id, coordinates: entity.coordinates.clone(), velocity: entity.velocity.clone(), yaw: entity.yaw });
        }
        for bullet in created_bullets.into_iter() {
            self.spawn_entity(bullet);
        }
    }

    async fn game_update_loop(&mut self, mut user_adder: mpsc::Receiver<WebSocketStream<TcpStream>>) {
        let mut tiles = PlayerPositions::new(self.config.map_size / 10.);
        let mut interval = time::interval(Duration::from_millis(self.config.update_delay_ms));
        let mut tick = 0u32;
        let (update_sender, mut received_updates) = mpsc::channel(1);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    self.update_entities(&mut tiles, tick).await;
                    self.dispatch_events().await;
                    self.queued_events.clear();
                    tick += 1;
                },
                message = user_adder.recv() => {
                    match message {
                        Some(stream) => self.spawn_player(stream, update_sender.clone()),
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
                        UserMessage::GoingAway(id) => self.kick_player(id)
                    }
                }
            }
        }
    }

    async fn dispatch_events(&self) {
        let data = bincode::serialize(&self.queued_events).unwrap();
        for (_, client) in self.clients.iter() {
            if let Err(_) = client.send(Message::Binary(data.clone())).await {
                warn!("Sent message to a client that isn't receiving messages");
            }
        }
    }

    fn kick_player(&mut self, id: Id) {
        self.clients.remove(&id);
        self.remove_entity(id);
    }

    fn remove_entity(&mut self, id: Id) {
        if self.entities.remove(&id).is_none() {
            warn!("Tried removing an entity that does not exist: {}", id);
            return;
        }
        self.queued_events.push(ServerEvent::EntityDelete(id));
    }

    fn spawn_entity(&mut self, entity: Entity) {
        self.queued_events.push(ServerEvent::EntityCreate { id: entity.id, tank: entity.tank.id, position: entity.coordinates.clone() });
        self.entities.insert(entity.id, entity);
    }

    fn spawn_player(&mut self, stream: WebSocketStream<TcpStream>, updates: mpsc::Sender<UserMessage>) {
        let (sender, receiver) = mpsc::channel::<Message>(1);
        let id = self.next_id();
        self.clients.insert(id, sender);
        let entity_data = Entity::new(Vector::empty(), id, &self.config, EntityType::Player(Player { points: 0, score: 0 }));
        self.spawn_entity(entity_data);
        tokio::spawn(handle_client_connection(stream, receiver, updates, id));
    }
}

type Tile = HashSet<Id>;

struct PlayerPositions {
    tiles: [[Tile; 10]; 10],
    scale: f64
}

impl PlayerPositions {

    fn new(scale: f64) -> Self {
        Self {
            tiles: array::from_fn(|_| array::from_fn(|_| HashSet::new())),
            scale
        }
    }

    fn get(&mut self, coords: &Vector) -> &mut Tile {
        &mut self.tiles[coords.y as usize / self.scale as usize][coords.x as usize / self.scale as usize]
    }

    fn add(&mut self, coords: &Vector, id: Id) -> bool {
        self.get(coords).insert(id)
    }

    fn remove(&mut self, coords: &Vector, id: Id) {
        self.get(coords).remove(&id);
    }
}