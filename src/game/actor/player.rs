use game::*;
use game::actor::*;
use na::Vector2;


#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub enum PlayerDirective {
    MovingToward(Target),
    FiringAt(Target),
    Idle,
}
impl PlayerDirective {
    pub fn is_moving(&self) -> bool {
        match *self {
            PlayerDirective::MovingToward(_) => true,
            _ => false
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PlayerInput {
    pub from: EntityId,
    pub set_directive: PlayerDirective,
}

/// Static attributes of a "player".
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct PlayerAttribsMemo {
    pub max_speed: f64,
    pub color: Color,
    pub fire_cooldown: f64,
}

/// Dynamic attributes of a "player".
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct PlayerStateMemo {
    pub pos: Vec2,
    pub directive: PlayerDirective,
    pub remaining_fire_cooldown: f64,
}

#[derive(Debug)]
pub struct PlayerActor {
    color: Color,
    pos: Vector2<f64>,
    max_speed: f64,
    directive: PlayerDirective,
    fire_cooldown: f64,
    remaining_fire_cooldown: f64,
}

impl PlayerActor {
    pub fn new(color: Color, pos: Vec2) -> PlayerActor {
        PlayerActor {
            color,
            pos: Vector2::new(pos.x, pos.y),
            max_speed: 300.0,
            directive: PlayerDirective::Idle,
            fire_cooldown: 0.2,
            remaining_fire_cooldown: 0.0,
        }
    }

    fn to_state_memo(&self) -> PlayerStateMemo {
        PlayerStateMemo {
            pos: (&self.pos).into(),
            directive: self.directive,
            remaining_fire_cooldown: self.remaining_fire_cooldown,
        }
    }

    fn to_attribs_memo(&self) -> PlayerAttribsMemo {
        PlayerAttribsMemo {
            max_speed: self.max_speed,
            color: self.color,
            fire_cooldown: self.fire_cooldown,
        }
    }
}

impl GameActor for PlayerActor {
    fn to_init_memo(&self) -> ActorInitMemo {
        ActorInitMemo::Player(
            self.to_state_memo(),
            self.to_attribs_memo()
        )
    }

    fn on_spawn(&mut self) -> () {
        println!("Spawn {:?}", *self);
    }

    fn on_player_input(&mut self, input: &PlayerInput) -> () {
        self.directive = input.set_directive;
    }

    fn tick<'a>(&mut self, dt: f64, game_handle: ActorGameHandle<'a>) -> Option<EntityState> {

        // no matter the state, decrement the fire cooldown
        if self.remaining_fire_cooldown > 0.0 {
            self.remaining_fire_cooldown -= dt;
        }

        // behave according to the current directive
        let current_directive = self.directive;
        match current_directive {
            PlayerDirective::Idle => (),
            PlayerDirective::MovingToward(ref target) => {
                let target_pos = game_handle.resolve_target_pos(target).resolve(self);
                match target_pos {
                    None => {
                        // player moving towards an entity with no position? Set it back to Idle
                        self.directive = PlayerDirective::Idle;
                    }
                    Some(ref dest) => {
                        let dest = dest.to_vec();
                        let mut trajectory = dest - self.pos;
                        let speed = self.max_speed * dt;
                        match trajectory.try_normalize_mut(speed) {
                            None => {
                                // destination will be reached this tick
                                self.pos.copy_from(&dest);
                                self.directive = PlayerDirective::Idle;
                            }
                            Some(_) => {
                                // update the position by scaling the trajectory by the speed
                                trajectory *= speed;
                                self.pos += trajectory;
                            }
                        }
                    }
                };
            }
            PlayerDirective::FiringAt(target) => {
                self.remaining_fire_cooldown -= dt;
                if self.remaining_fire_cooldown <= 0.0 {
                    // fire!
                    self.remaining_fire_cooldown = self.fire_cooldown;
                    let target_pos = game_handle.resolve_target_pos(&target).resolve(self);
                    match target_pos {
                        None => println!("whiff!"),
                        Some(ref target) => {
                            let bullet = bullet::BulletActor::new(self.pos.clone(), self.color, target);
                            game_handle.spawn(Box::new(bullet));
                        }
                    }
                }
            }
        };

        // return a memo of the updated state
        Some(EntityState::Player(self.to_state_memo()))
    }

    fn get_pos(&self) -> Vec2 {
        (&self.pos).into()
    }
}