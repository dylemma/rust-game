pub mod player;
pub mod bullet;

use game::*;
use std::ops::Deref;

pub trait GameActor {
    fn to_init_memo(&self) -> ActorInitMemo;
    fn on_spawn(&mut self) -> ();
    fn on_player_input(&mut self, input: &PlayerInput) -> ();
    fn tick<'a>(&mut self, dt: f64, game_handle: ActorGameHandle<'a>) -> Option<EntityState>;
    fn get_pos(&self) -> Vec2;
}

pub struct ActorRef {
    actor: RefCell<Box<GameActor>>,
}

impl ActorRef {
    pub fn new(actor: Box<GameActor>) -> ActorRef {
        ActorRef { actor: RefCell::new(actor) }
    }
    pub fn to_init_memo(&self) -> ActorInitMemo {
        self.actor.borrow().to_init_memo()
    }
    pub fn get_pos(&self) -> Vec2 {
        self.actor.borrow().deref().get_pos()
    }
    pub fn on_player_input(&self, input: &PlayerInput) -> () {
        self.actor.borrow_mut().on_player_input(input);
    }
    pub fn tick<'a>(&self, dt: f64, game_handle: ActorGameHandle<'a>) -> Option<EntityState> {
        self.actor.borrow_mut().tick(dt, game_handle)
    }
}


#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub enum ActorInitMemo {
    Player(self::player::PlayerStateMemo, self::player::PlayerAttribsMemo),
    Bullet(self::bullet::BulletStateMemo, self::bullet::BulletAttribsMemo),
}
impl ActorInitMemo {
    pub fn set_state(&mut self, state: EntityState) -> () {
        match (*self, state) {
            (ActorInitMemo::Player(_, attribs), EntityState::Player(state)) => *self = ActorInitMemo::Player(state, attribs),
            (ActorInitMemo::Bullet(_, attribs), EntityState::Bullet(state)) => *self = ActorInitMemo::Bullet(state, attribs),
            (memo, state) => panic!("State cannot be applied to this object: {:?} into {:?}", state, memo)
        }
    }
}

/// Stores only the "dynamic" attributes of an entity.
/// Used to reduce the number of bytes sent to clients for regular state updates.
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub enum EntityState {
    Player(PlayerStateMemo),
    Bullet(BulletStateMemo),
}