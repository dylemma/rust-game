use futures::{Future, Poll, Stream};
use futures::sync::mpsc::{unbounded as stream_channel, UnboundedSender, UnboundedReceiver};

use rand::{Rand, Rng, thread_rng};

use std::rc::Rc;
use std::sync::mpsc::{channel, Sender, SendError, Receiver};
use na::{Vector2};
use std::time::{Duration, Instant};
use futures::sync::oneshot;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use rand::ThreadRng;
use std::cell::{Ref, RefCell};
use std::ops::{Deref, DerefMut};
use std::thread;
use tokio_timer::{Timer, wheel};

pub type EntityId = u64;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Entity {
    pub id: EntityId,
    pub data: EntityAttribs,
}

/// Stores the "static" and "dynamic" attributes for entities, i.e. Players and Bullets.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum EntityAttribs {
    Player {
        attributes: PlayerData,
        state: RefCell<PlayerState>,
    },
    Bullet {
        attributes: BulletData,
        state: RefCell<BulletState>,
    },
}
impl EntityAttribs {
    fn get_pos(&self) -> Option<Vec2> {
        match *self {
            EntityAttribs::Player { ref state, .. } => Some(state.borrow().pos),
            EntityAttribs::Bullet { ref state, .. } => Some(state.borrow().pos),
        }
    }
//    fn copy_state(&self) -> EntityState {
//        match *self {
//            EntityAttribs::Player { state, .. } => EntityState::Player(state),
//            EntityAttribs::Bullet { state, .. } => EntityState::Bullet(state),
//        }
//    }
//    fn borrow_state<'a>(&'a self) -> Ref<'a, EntityState> {
//        match *self {
//            EntityAttribs::Player { ref state, .. } => Ref::map(state.borrow(), |ps| &'a EntityState::Player(*ps)),
//            EntityAttribs::Bullet { ref state, .. } => Ref::map(state.borrow(), |bs| &'a EntityState::Bullet(*bs)),
//        }
//    }
    fn borrow_state<F, U>(&self, f: F) -> U
        where F: Fn(EntityState) -> U
    {
        match *self {
            EntityAttribs::Player { ref state, .. } => {
                let player_state = state.borrow();
                let ps = player_state.deref();
                f(EntityState::Player(*ps))
            },
            EntityAttribs::Bullet { ref state, .. } => {
                let bullet_state = state.borrow();
                let bs = bullet_state.deref();
                f(EntityState::Bullet(*bs))
            }
        }
    }

    pub fn set_state(&self, new_state: EntityState) {
        match (self, new_state) {
            (&EntityAttribs::Player { ref state, .. }, EntityState::Player(ps)) => {
                let mut s = state.borrow_mut();
                *s = ps;
            },
            (&EntityAttribs::Bullet { ref state, .. }, EntityState::Bullet(bs)) => {
                let mut s = state.borrow_mut();
                *s = bs;
            },
            _ => {
                println!("invalid input to set_state (wrong type)");
            }
        }
    }
}

/// Stores only the "dynamic" attributes of an entity.
/// Used to reduce the number of bytes sent to clients for regular state updates.
#[derive(Serialize, Deserialize, Debug)]
pub enum EntityState {
    Player(PlayerState),
    Bullet(BulletState),
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub enum Target {
    Location(Vec2),
    Entity(EntityId),
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct Color([f32; 3]);
impl Rand for Color {
    fn rand<R: Rng>(rng: &mut R) -> Self {
        Color([
            rng.next_f32(),
            rng.next_f32(),
            rng.next_f32()
        ])
    }
}
impl Color {
    pub fn to_vec4(&self) -> [f32; 4] {
        [self.0[0], self.0[1], self.0[2], 1.0]
    }
}

/// Static attributes of a "player".
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct PlayerData {
    pub speed: f64,
    pub color: Color,
    pub fire_cooldown: f64,
}

/// Dynamic attributes of a "player".
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct PlayerState {
    pub pos: Vec2,
    pub directive: PlayerDirective,
    pub remaining_fire_cooldown: f64,
}

/// Static attributes of a "bullet"
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct BulletData {
    vel: Vec2,
    color: Color,
}

/// Dynamic attributes of a "bullet"
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct BulletState {
    pos: Vec2,
    remaining_life: Duration,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct Vec2{
    pub x: f64,
    pub y: f64,
}
impl Vec2 {
    pub fn new(x: f64, y: f64) -> Vec2 {
        Vec2{ x, y }
    }
    #[allow(unused)]
    fn load(&mut self, v: Vector2<f64>) {
        self.x = v.x;
        self.y = v.y;
    }
    #[allow(unused)]
    fn store(&self, v: &mut Vector2<f64>) {
        v.x = self.x;
        v.y = self.y;
    }
    fn to_vec(&self) -> Vector2<f64> {
        Vector2::from([self.x, self.y])
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub enum PlayerDirective {
    MovingToward(Target),
    FiringAt(Target),
    Idle,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PlayerInput {
    pub from: EntityId,
    pub set_directive: PlayerDirective,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum GameStateUpdate {
    SetClientId(EntityId),
    Spawn(Entity),
    Despawn(EntityId),
    Update(EntityId, EntityState)
}

pub struct ConnectedPlayer {
    pawn_id: EntityId,
    tx: UnboundedSender<GameStateUpdate>,
    needs_inits: bool,
}

enum ConnectionStateChange {
    NewConnection {
        entity_id_channel: oneshot::Sender<EntityId>,
        updates_channel: UnboundedSender<GameStateUpdate>
    },
    Dropped(EntityId)
}

#[derive(Serialize, Deserialize, Debug)]
pub enum GameInput {
    Input(PlayerInput),
}

trait Sleeper {
    fn sleep(&self, dur: Duration) -> ();
}

/// Seemingly the most accurate and relatively-low CPU-using sleep implementation.
/// Comparing with `thread::sleep`, the accuracy is better (when configured right).
/// Comparing with a CPU-bound loop, the accuracy is a bit worse, but obviously it
/// uses much less CPU power, which I think is more important for running the game loop.
struct TokioTimer(Timer);
impl Sleeper for TokioTimer {
    fn sleep(&self, dur: Duration) -> () {
        self.0.sleep(dur).wait().unwrap();
    }
}

pub struct GameHandle {
    input_sender: Sender<GameInput>,
    connection_sender: Sender<ConnectionStateChange>,
}
impl GameHandle {
    pub fn send(&self, input: GameInput) -> Result<(), SendError<GameInput>> {
        self.input_sender.send(input)
    }
    pub fn new_connection(&self, updates_channel: UnboundedSender<GameStateUpdate>) -> Result<oneshot::Receiver<EntityId>, SendError<UnboundedSender<GameStateUpdate>>> {
        let (sender, receiver) = oneshot::channel();
        let msg = ConnectionStateChange::NewConnection {
            entity_id_channel: sender,
            updates_channel
        };
        self.connection_sender.send(msg)
            .map_err(|msg| {
                SendError(match msg.0 {
                    ConnectionStateChange::NewConnection { updates_channel, .. } => updates_channel,
                    _ => unreachable!(),
                })
            })
            .map(|_| receiver)
    }
    pub fn dropped_connection(&self, entity_id: EntityId) -> Result<(), SendError<()>> {
        let msg = ConnectionStateChange::Dropped(entity_id);
        self.connection_sender.send(msg).map_err(|_| SendError(()))
    }
}



pub struct Game {
    next_entity_id: EntityId,
    clients: Vec<ConnectedPlayer>,
    entity_ids: BTreeSet<EntityId>,
    entities: BTreeMap<EntityId, Entity>,
    spawn_queue: VecDeque<Entity>,
    despawn_queue: VecDeque<EntityId>,
    inputs: Receiver<GameInput>,
    connections_changes: Receiver<ConnectionStateChange>,
}
impl Game {
    pub fn new() -> (Game, GameHandle) {
        let (input_sender, input_receiver) = channel();
        let (connection_sender, connection_receiver) = channel();
        let game = Game {
            next_entity_id: 1,
            clients: Vec::new(),
            entity_ids: BTreeSet::new(),
            entities: BTreeMap::new(),
            spawn_queue: VecDeque::new(),
            despawn_queue: VecDeque::new(),
            inputs: input_receiver,
            connections_changes: connection_receiver,
        };
        (game, GameHandle { input_sender, connection_sender })
    }

    pub fn run(&mut self) {
        let sleepy = TokioTimer(wheel()
            .tick_duration(Duration::from_millis(1))
            .max_timeout(Duration::from_millis(10))
            .build());

        let mut last_update_time = Instant::now();
        let target_tick_rate = Duration::from_millis(8);
        let dt = 0.008;

        let mut countdown = 200;
        loop {
            let before_tick = Instant::now();
            let next_tick_time = before_tick + target_tick_rate;
            self.tick(dt);
            let after_tick = Instant::now();
            let tick_time = after_tick - before_tick;

            // sleep until the next_tick_time
            let sleep_time = {
                if next_tick_time > after_tick {
                    let sleep_duration = next_tick_time - after_tick;
                    sleepy.sleep(sleep_duration);
                    let after_sleep = Instant::now();
                    after_sleep - after_tick
                } else {
                    Duration::new(0, 0)
                }
            };

            if cfg!(server_tick_stats = "true") {
                println!("Tick [{}s {}ns], Sleep [{}s {}ns]", tick_time.as_secs(), tick_time.subsec_nanos(), sleep_time.as_secs(), sleep_time.subsec_nanos());
            }

        }
    }

    #[allow(unused)]
    pub fn tick(&mut self, dt: f64) {
        self.handle_connections();
        self.handle_inputs();

        while let Some(to_despawn) = self.despawn_queue.pop_front() {
            let did_despawn = self.entities.remove(&to_despawn).is_some();
            self.entity_ids.remove(&to_despawn);

            for client in self.clients.iter() {
                client.tx.unbounded_send(GameStateUpdate::Despawn(to_despawn));
            }
        }

        let mut scratch_vec_1 = Vector2::from([0.0, 0.0]);
        let mut scratch_vec_2 = Vector2::from([0.0, 0.0]);

        // tick the state of each entity that had already spawned
        for id in self.entity_ids.iter() {
            let entity = self.entities.get(id).unwrap();
            match entity.data {
                EntityAttribs::Player { ref attributes, ref state } => {
                    let mut state = state.borrow_mut();
                    let directive = state.directive;
                    match directive {
                        PlayerDirective::Idle => (),
                        PlayerDirective::MovingToward(ref target) => {
                            let target_pos = self.resolve_target_pos(entity,target);
                            match target_pos {
                                None => {
                                    // player moving towards an entity with no position? Set it back to Idle
                                    state.directive = PlayerDirective::Idle;
                                },
                                Some(ref dest) => {
                                    let dest = dest.to_vec();
                                    let mut pos = state.pos.to_vec();
                                    let mut trajectory = dest - pos;
                                    let speed = attributes.speed * dt;
                                    match trajectory.try_normalize_mut(speed) {
                                        None => {
                                            // destination will be reached this tick
                                            state.pos.load(dest);
                                            state.directive = PlayerDirective::Idle;
                                            println!("Entity {} reached its destination ({:?}) and became Idle", id, state.pos);
                                        },
                                        Some(_) => {
                                            // update the position by scaling the trajectory by the speed
                                            println!("Entity {} moving towards {:?} from {:?} at speed {}", id, dest, state.pos, speed);
                                            trajectory *= speed;
                                            pos += trajectory;
                                            state.pos.load(pos);
                                        }
                                    }
                                }
                            }
                        },
                        PlayerDirective::FiringAt(target) => ()
                    }
                },
                EntityAttribs::Bullet { ref attributes, ref state } => (),
            };
        }

        // let all connected players know the current state
        for (id, entity) in self.entities.iter() {
            for client in self.clients.iter() {
                if client.needs_inits {
                    let e: Entity = (*entity).clone();
                    client.tx.unbounded_send(GameStateUpdate::Spawn(e));
                } else {
                    let id = entity.id;
                    let state = entity.data.borrow_state(|s| s);
                    client.tx.unbounded_send(GameStateUpdate::Update(id, state));
                }
            }
        }

        // clear the `needs_inits` flag for all clients, now that we have sent any necessary init messages
        for client in self.clients.iter_mut() {
            client.needs_inits = false;
        }

        // pump the spawn queue into the entity collection, and let clients know
        while let Some(entity) = self.spawn_queue.pop_front() {
            println!("Spawn {:?}", entity);
            self.entity_ids.insert(entity.id);
            self.entities.insert(entity.id, entity.clone());

            for client in self.clients.iter() {
                client.tx.unbounded_send(GameStateUpdate::Spawn(entity.clone()));
            }
        }

    }

    fn resolve_target_pos(&self, from: &Entity, target: &Target) -> Option<Vec2> {
        match *target {
            Target::Location(vec) => Some(vec),
            Target::Entity(id) if id == from.id => from.data.get_pos(),
            Target::Entity(id) => self.entities.get(&id).and_then(|ref entity| entity.data.get_pos())
        }
    }

    fn handle_connections(&mut self) {
        for connection_state_change in self.connections_changes.try_iter() {
            match connection_state_change {
                ConnectionStateChange::NewConnection { entity_id_channel, updates_channel } => {
                    let entity_id = self.next_entity_id;
                    self.next_entity_id += 1;

                    match entity_id_channel.send(entity_id) {
                        Err(unsent_id) => {
                            // connection must have dropped already. Don't add the entity; reuse the id.
                            self.next_entity_id = unsent_id;
                        },
                        Ok(()) => {
                            // add a new client based on the connection information given
                            self.clients.push(ConnectedPlayer {
                                pawn_id: entity_id,
                                tx: updates_channel,
                                needs_inits: true
                            });

                            // create a new Player entity for the connected player
                            let entity = Entity {
                                id: entity_id,
                                data: EntityAttribs::Player {
                                    attributes: PlayerData {
                                        speed: 300.0,
                                        color: thread_rng().gen(),
                                        fire_cooldown: 0.2
                                    },
                                    state: RefCell::new(PlayerState {
                                        pos: Vec2::new(0.0, 0.0),
                                        directive: PlayerDirective::Idle,
                                        remaining_fire_cooldown: 0.0
                                    })
                                }
                            };

                            // Add the entity to the queue, providing the "entity_id_channel" as a means
                            // of notifying the sender of this message that the entity has spawned.
                            self.spawn_queue.push_back(entity);

                        },
                    }

                },
                ConnectionStateChange::Dropped(entity_id) => {
                    self.clients.retain(|client| client.pawn_id != entity_id);
                    self.despawn_queue.push_back(entity_id);
                }
            }
        }

    }

    #[allow(unused)]
    fn handle_inputs(&mut self) {
        for input in self.inputs.try_iter() {
            match input {
                // Handle an input from a player
                GameInput::Input(player_input) => {
                    match self.entities.get(&player_input.from) {
                        None => {
                            println!("Got a player input associated with an entity that doesn't exist.");
                        },
                        Some(entity) => {
                            let directive = player_input.set_directive;
                            match entity.data {
                                EntityAttribs::Player { ref state, .. } => {
                                    let mut state_mut = state.borrow_mut();
                                    state_mut.directive = directive;
                                },
                                _ => {
                                    println!("Got a player input associated with a non-player entity");
                                }
                            }
                        }
                    }
                },
            }
        }
    }

}

///-------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct GridPoint(pub i32, pub i32);
impl GridPoint {
    fn move_by(&mut self, d_pos: &GridPoint) {
        self.0 += d_pos.0;
        self.1 += d_pos.1;
    }
}
impl Rand for GridPoint {
    fn rand<R: Rng>(rng: &mut R) -> Self {
        let x = (rng.next_u32() % 21) as i32 - 10;
        let y = (rng.next_u32() % 21) as i32 - 10;
        GridPoint(x, y)
    }
}

pub type PlayerName = char;
pub type PlayerUid = u64;

pub struct Player {
    id: PlayerUid,
    name: PlayerName,
    pos: GridPoint,
    is_newly_connected: bool,
    is_modified: bool,
}
impl Player {
    pub fn new(id: PlayerUid, name: PlayerName, pos: GridPoint) -> Player {
        Player {
            id,
            name,
            pos,
            is_newly_connected: true,
            is_modified: false,
        }
    }
    fn move_by(&mut self, d_pos: &GridPoint) {
        self.is_modified = true;
        self.pos.move_by(d_pos);
    }
    fn send_state_updates(&mut self, out: &UnboundedSender<GridUpdate>, replay_inits: bool) {
        if self.is_newly_connected || replay_inits {
            out.unbounded_send(GridUpdate::Connected(self.id, self.name,self.pos)).unwrap();
        }
        if self.is_modified || replay_inits {
            out.unbounded_send(GridUpdate::MovedTo(self.id, self.pos)).unwrap();
        }
        self.is_newly_connected = false;
        self.is_modified = false;
    }
}

#[derive(Debug)]
pub enum GridMessage {
    Connect(PlayerUid, PlayerName, GridPoint),
    Disconnect(PlayerUid),
    MoveRel(PlayerUid, GridPoint),
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub enum GridUpdate {
    Connected(PlayerUid, PlayerName, GridPoint),
    MovedTo(PlayerUid, GridPoint),
    Disconnected(PlayerUid),
}

/// A stream of `GridUpdate` events produced by a `Grid`.
///
/// As the game loop causes things to move around and change state,
/// the `Grid` will emit events to this stream.
///
/// Because the stream has an unlimited buffer, it must be consumed
/// (i.e. polled) to avoid using too much memory.
#[must_use]
pub struct GridBroadcast {
    inner: UnboundedReceiver<GridUpdate>
}

impl Stream for GridBroadcast {
    type Item = GridUpdate;
    type Error = ();
    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        self.inner.poll()
    }
}

/// Entry point for passing `GridMessages` to a `Grid`.
///
/// Messages passed in this manner are the only means of sending user input
/// to the `Grid`. Each message will be handled during the next available
/// iteration of the game loop.
#[derive(Clone)]
pub struct GridInbox {
    inner: Rc<Sender<GridMessage>>,
}
impl GridInbox {
    pub fn send(&self, msg: GridMessage) -> Result<(), ()> {
        self.inner.send(msg).map_err(|_| ())
    }
}

pub struct Grid {
    players: Vec<Player>,
    incoming: Receiver<GridMessage>,
    outgoing: UnboundedSender<GridUpdate>,
}

impl Grid {
    pub fn new() -> (Grid, GridInbox, GridBroadcast) {
        let (grid_message_sender, grid_message_receiver) = channel();
        let (grid_update_sender, grid_update_receiver) = stream_channel();
        let grid = Grid {
            players: Vec::new(),
            incoming: grid_message_receiver,
            outgoing: grid_update_sender
        };
        let grid_inbox = GridInbox { inner: Rc::new(grid_message_sender) };
        let grid_broadcast = GridBroadcast{ inner: grid_update_receiver };
        (grid, grid_inbox, grid_broadcast)
    }
    pub fn run(&mut self) {
        let mut event_buf: Vec<GridMessage> = Vec::new();
        loop {
            event_buf.clear();
            event_buf.extend(self.incoming.try_iter());
            for event in event_buf.iter() {
                match *event {
                    GridMessage::Connect(id, name, pos) => {
                        let player = Player::new(id, name,pos);
                        self.players.push(player);
                    },
                    GridMessage::Disconnect(id) => {
                        self.players.retain(|p| p.id != id);
                        self.outgoing.unbounded_send(GridUpdate::Disconnected(id)).unwrap();
                    },
                    GridMessage::MoveRel(id, ref d_pos) => {
                        for player in self.players.iter_mut() {
                            if player.id == id {
                                player.move_by(d_pos);
                            }
                        }
                    },
                }
            }
            for player in self.players.iter_mut() {
                player.send_state_updates(&self.outgoing, false);
            }
        };
    }
}
