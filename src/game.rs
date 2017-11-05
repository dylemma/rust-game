use futures::Future;
use futures::sync::mpsc::UnboundedSender;
use futures::sync::oneshot;

use na::{Vector2};
use rand::{Rand, Rng, thread_rng};

use std::cell::{RefCell};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ops::Deref;
use std::sync::{Arc, Mutex, Condvar};
use std::sync::mpsc::{channel, Sender, SendError, Receiver};
use std::thread;
use std::time::{Duration, Instant};

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
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
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
    pub max_speed: f64,
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
    Spawn(EntityId, ActorInitMemo),
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

enum ActorQueryResult<T> {
    Found(T),
    NotFound,
    Yourself,
}

struct PositionQueryResult(ActorQueryResult<Vec2>);
impl PositionQueryResult {
    pub fn resolve(self, yourself: &GameActor) -> Option<Vec2> {
        match self.0 {
            ActorQueryResult::Found(pos) => Some(pos),
            ActorQueryResult::NotFound => None,
            ActorQueryResult::Yourself => Some(yourself.get_pos()),
        }
    }
}

struct ActorGameHandle<'a> {
    id: EntityId,
    game: &'a Game,
}
impl <'a> ActorGameHandle<'a> {
    fn resolve_target_pos(&self, target: &Target) -> PositionQueryResult {
        use self::ActorQueryResult::*;
        let inner_result = match *target {
            Target::Location(vec) => Found(vec),
            Target::Entity(id) if id == self.id => Yourself,
            Target::Entity(id) => match self.game.actors.get(&id) {
                None => NotFound,
                Some(ref actor_ref) => Found(actor_ref.get_pos()),
            }
        };
        PositionQueryResult(inner_result)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub enum ActorInitMemo {
    Player(PlayerState, PlayerData),
}
impl ActorInitMemo {
    pub fn set_state(&mut self, state: EntityState) -> () {
        match (*self, state) {
            (ActorInitMemo::Player(_, data), EntityState::Player(player_state)) => *self = ActorInitMemo::Player(player_state, data),
            (memo, state) => panic!("State cannot be applied to this object: {:?} into {:?}", state, memo)
        }
    }
}

trait GameActor {
    fn to_init_memo(&self) -> ActorInitMemo;
    fn on_spawn(&mut self) -> ();
    fn on_player_input(&mut self, input: &PlayerInput) -> ();
    fn tick<'a>(&mut self, dt: f64, game_handle: ActorGameHandle<'a>) -> Option<EntityState>;
    fn get_pos(&self) -> Vec2;
}

struct ActorRef {
    actor: RefCell<Box<GameActor>>,
}
impl ActorRef {
    fn to_init_memo(&self) -> ActorInitMemo {
        self.actor.borrow().to_init_memo()
    }
    fn get_pos(&self) -> Vec2 {
        self.actor.borrow().deref().get_pos()
    }
    fn on_player_input(&self, input: &PlayerInput) -> () {
        self.actor.borrow_mut().on_player_input(input);
    }
    fn tick<'a>(&self, dt: f64, game_handle: ActorGameHandle<'a>) -> Option<EntityState> {
        self.actor.borrow_mut().tick(dt, game_handle)
    }
}

#[derive(Debug)]
struct PlayerActor {
    state: PlayerState,
    attributes: PlayerData,
}


impl GameActor for PlayerActor {
    fn to_init_memo(&self) -> ActorInitMemo {
        ActorInitMemo::Player(self.state, self.attributes)
    }

    fn on_spawn(&mut self) -> () {
        println!("Spawn {:?}", *self);
    }

    fn on_player_input(&mut self, input: &PlayerInput) -> () {
        self.state.directive = input.set_directive;
    }

    fn tick<'a>(&mut self, dt: f64, game_handle: ActorGameHandle<'a>) -> Option<EntityState> {
        let current_directive = self.state.directive;
        match current_directive {
            PlayerDirective::Idle => {
                // nothing will change
                None
            },
            PlayerDirective::MovingToward(ref target) => {
                let target_pos = game_handle.resolve_target_pos(target).resolve(self);
                match target_pos {
                    None => {
                        // player moving towards an entity with no position? Set it back to Idle
                        self.state.directive = PlayerDirective::Idle;
                    },
                    Some(ref dest) => {
                        let dest = dest.to_vec();
                        let mut pos = self.state.pos.to_vec();
                        let mut trajectory = dest - pos;
                        let speed = self.attributes.max_speed * dt;
                        match trajectory.try_normalize_mut(speed) {
                            None => {
                                // destination will be reached this tick
                                self.state.pos.load(dest);
                                self.state.directive = PlayerDirective::Idle;
                            },
                            Some(_) => {
                                // update the position by scaling the trajectory by the speed
                                trajectory *= speed;
                                pos += trajectory;
                                self.state.pos.load(pos);
                            }
                        }
                    }
                };
                Some(EntityState::Player(self.state))
            },
            PlayerDirective::FiringAt(target) => {
                // TODO: implement firing directive
                unimplemented!();
                Some(EntityState::Player(self.state))
            }
        }
    }

    fn get_pos(&self) -> Vec2 {
        self.state.pos
    }

}

/// Handle for starting the game loop thread.
///
/// We want to be able to run the game loop on the same thread where the game was started,
/// since the game contains non-`Send`able values which would cause compilation errors if
/// we tried to create a game and then move it onto a new thread.
///
/// This struct has a single method, `start`, which consumes the handle and un-blocks the
/// thread that was started by the Game constructor that created this handle.
pub struct GameStartHandle {
    loc_var_pair: Arc<(Mutex<bool>, Condvar)>
}
impl GameStartHandle {
    pub fn start(self) -> () {
        let &(ref lock, ref cvar) = &*self.loc_var_pair;
        let mut started = lock.lock().unwrap();
        *started = true;
        cvar.notify_one();
    }
}

pub struct Game {
    next_entity_id: EntityId,
    clients: Vec<ConnectedPlayer>,
    entity_ids: BTreeSet<EntityId>,
    actors: BTreeMap<EntityId, ActorRef>,
    spawn_queue: VecDeque<(EntityId, Box<GameActor>)>,
    despawn_queue: VecDeque<EntityId>,
    inputs: Receiver<GameInput>,
    connections_changes: Receiver<ConnectionStateChange>,
}
impl Game {
    pub fn new() -> (GameStartHandle, GameHandle) {
        let (input_sender, input_receiver) = channel();
        let (connection_sender, connection_receiver) = channel();

        let pair = Arc::new((Mutex::new(false), Condvar::new()));
        let pair2 = pair.clone();

        thread::spawn(move || {
            let &(ref lock, ref cvar) = &*pair2;
            let mut started = lock.lock().unwrap();
            while !*started {
                started = cvar.wait(started).unwrap();
            }

            let mut game = Game {
                next_entity_id: 1,
                clients: Vec::new(),
                entity_ids: BTreeSet::new(),
                actors: BTreeMap::new(),
                spawn_queue: VecDeque::new(),
                despawn_queue: VecDeque::new(),
                inputs: input_receiver,
                connections_changes: connection_receiver,
            };

            game.run();
        });

        let gsh = GameStartHandle { loc_var_pair: pair };

        (gsh, GameHandle { input_sender, connection_sender })
    }

    pub fn run(&mut self) {
        let sleepy = TokioTimer(wheel()
            .tick_duration(Duration::from_millis(1))
            .max_timeout(Duration::from_millis(10))
            .build());

        let target_tick_rate = Duration::from_millis(8);
        let dt = 0.008;

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
            let did_despawn = self.actors.remove(&to_despawn).is_some();
            self.entity_ids.remove(&to_despawn);

            for client in self.clients.iter() {
                client.tx.unbounded_send(GameStateUpdate::Despawn(to_despawn));
            }
        }

        let mut scratch_vec_1 = Vector2::from([0.0, 0.0]);
        let mut scratch_vec_2 = Vector2::from([0.0, 0.0]);

        // Update the state of each actor
        for id in self.entity_ids.iter() {
            let mut actor = self.actors.get(id).unwrap();
            let handle = ActorGameHandle { id: *id, game: self };
            let updated = actor.tick(dt, handle);

            // send the state update to all interested actors,
            // or send the full init to actors that need it
            for client in self.clients.iter() {
                if client.needs_inits {
                    let init_memo = actor.to_init_memo();
                    client.tx.unbounded_send(GameStateUpdate::Spawn(*id, init_memo)).unwrap();
                } else {
                    match updated {
                        Some(state) => {
                            client.tx.unbounded_send(GameStateUpdate::Update(*id, state)).unwrap();
                        },
                        None => (),
                    };
                }
            }
        }

        // clear the `needs_inits` flag for all clients, now that we have sent any necessary init messages
        for client in self.clients.iter_mut() {
            client.needs_inits = false;
        }

        // pump the spawn queue into the entity collection, and let clients know
        while let Some((id, mut entity)) = self.spawn_queue.pop_front() {
            entity.on_spawn();
            self.entity_ids.insert(id);
            let init_memo = entity.to_init_memo();
            self.actors.insert(id, ActorRef {
                actor: RefCell::new(entity),
            });

            for client in self.clients.iter() {
                client.tx.unbounded_send(GameStateUpdate::Spawn(id, init_memo));
            }
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
                            let player_actor = PlayerActor {
                                attributes: PlayerData {
                                    max_speed: 300.0,
                                    color: thread_rng().gen(),
                                    fire_cooldown: 0.2
                                },
                                state: PlayerState {
                                    pos: Vec2::new(0.0, 0.0),
                                    directive: PlayerDirective::Idle,
                                    remaining_fire_cooldown: 0.0
                                }
                            };

                            // Add the entity to the spawn queue
                            self.spawn_queue.push_back((entity_id, Box::new(player_actor)));

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
                    match self.actors.get(&player_input.from) {
                        None => {
                            println!("Got a player input associated with an entity that doesn't exist.");
                        },
                        Some(actor) => {
                            actor.on_player_input(&player_input);
                        }
                    }
                },
            }
        }
    }

}
