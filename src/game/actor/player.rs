use game::*;
use game::actor::*;


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

#[derive(Debug)]
pub struct PlayerActor {
    attributes: PlayerData,
    state: PlayerState,
}

impl PlayerActor {
    pub fn new(attributes: PlayerData, state: PlayerState) -> PlayerActor {
        PlayerActor { attributes, state }
    }
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
            }
            PlayerDirective::MovingToward(ref target) => {
                let target_pos = game_handle.resolve_target_pos(target).resolve(self);
                match target_pos {
                    None => {
                        // player moving towards an entity with no position? Set it back to Idle
                        self.state.directive = PlayerDirective::Idle;
                    }
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
                            }
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
            }
            PlayerDirective::FiringAt(target) => {
                // TODO: implement firing directive
                // Some(EntityState::Player(self.state))
                unimplemented!();
            }
        }
    }

    fn get_pos(&self) -> Vec2 {
        self.state.pos
    }
}