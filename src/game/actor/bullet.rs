use game::*;
use game::actor::*;
use na::Vector2;

use std::time::{Duration, Instant};

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct BulletStateMemo {
    pub pos: Vec2,
}
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct BulletAttribsMemo {
    pub color: Color,
    pub vel: Vec2,
}

#[derive(Debug)]
pub struct BulletActor {
    color: Color,
    pos: Vector2<f64>,
    vel: Vector2<f64>,
    scratch_vec: Vector2<f64>,
    seconds_til_expire: f64,
}

impl BulletActor {
    pub fn new(pos: Vector2<f64>, color: Color, toward: &Vec2) -> BulletActor {
        /// vel = (target - playerpos).normalized * 1000
        let mut vel = toward.to_vec();
        vel -= pos;
        if vel.try_normalize_mut(0.001).is_none() {
            vel.x = 1.0;
            vel.y = 0.0;
        }
        vel *= 800.0;

        BulletActor {
            color,
            pos,
            vel,
            scratch_vec: Vector2::new(0.0, 0.0),
            seconds_til_expire: 1.2,
        }
    }
}

impl GameActor for BulletActor {
    fn to_init_memo(&self) -> ActorInitMemo {
        ActorInitMemo::Bullet(
            BulletStateMemo {
                pos: self.get_pos(),
            },
            BulletAttribsMemo {
                color: self.color,
                vel: (&self.vel).into()
            }
        )
    }

    fn on_spawn(&mut self) -> () {}
    fn on_player_input(&mut self, input: &PlayerInput) -> () {}

    fn get_pos(&self) -> Vec2 {
        (&self.pos).into()
    }

    fn tick<'a>(&mut self, dt: f64, game_handle: ActorGameHandle<'a>) -> Option<EntityState> {
        // calculate the "velocity step", which is the velocity scaled by the time step
        let mut vel_step = self.scratch_vec;
        vel_step.copy_from(&self.vel);
        vel_step *= dt;

        // move the position by the velocity step
        self.pos += vel_step;

        self.seconds_til_expire -= dt;
        if self.seconds_til_expire < 0.0 {
            game_handle.despawn_self();
        }

        Some(EntityState::Bullet(BulletStateMemo {
            pos: self.get_pos()
        }))
    }
}