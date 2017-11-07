use binio::*;
use game::*;
use game::actor::*;
use game::actor::player::*;

use futures::{future, Future, Sink, Stream};
use futures::sync::mpsc::{unbounded as stream_channel};

use na::Vector2;

use graphics::ellipse;
use piston_window::*;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::mpsc::channel;
use std::thread;

use tokio_core::net::TcpStream;
use tokio_core::reactor::Core;

struct GameClientIOMessages;
impl IOMessages for GameClientIOMessages {
    type Input = GameStateUpdate;
    type Output = GameInput;
}

/// Raw inputs from the player's keyboard and mouse
struct PlayerControlState {
    mouse_pos: Vec2,
    mouse_down: bool,
    ctrl_down: bool,
}
impl PlayerControlState {
    fn new() -> PlayerControlState {
        PlayerControlState{
            mouse_pos: Vec2::new(0.0, 0.0),
            mouse_down: false,
            ctrl_down: false
        }
    }
}

/// Interpreted inputs from the player.
///
/// Keeps track of the raw state as well as the previous "directive",
/// to allow for "sticky" states that are not reset when controls are released.
struct PlayerControls {
    state: PlayerControlState,
    directive: PlayerDirective
}
impl PlayerControls {
    fn new() -> PlayerControls {
        PlayerControls {
            state: PlayerControlState::new(),
            directive: PlayerDirective::Idle
        }
    }

    fn check_directive_change(&mut self, event: &Event) -> Option<PlayerDirective> {
        let mut hit_event = false;

        // track mouse position
        event.mouse_cursor(|x, y| {
            self.state.mouse_pos.x = x;
            self.state.mouse_pos.y = y;
            hit_event = true;
        });

        // reset button states if the window loses focus
        event.focus(|is_focused| {
            if !is_focused {
                self.state.mouse_down = false;
                self.state.ctrl_down = false;
                hit_event = true;
            }
        });

        // watch for keyboard/mouse activity
        event.button(|btn| {
            match btn.button {
                // left click
                Button::Mouse(MouseButton::Left) => {
                    hit_event = true;
                    self.state.mouse_down = match btn.state {
                        ButtonState::Press => true,
                        ButtonState::Release => false,
                    };
                },
                // ctrl key
                Button::Keyboard(Key::LCtrl) => {
                    hit_event = true;
                    self.state.ctrl_down = match btn.state {
                        ButtonState::Press => true,
                        ButtonState::Release => false,
                    }
                }
                _ => ()
            }
        });

        if hit_event {
            let new_directive = {
                let PlayerControlState { mouse_pos, mouse_down, ctrl_down } = self.state;
                if mouse_down {
                    let target = Target::Location(mouse_pos);
                    if ctrl_down {
                        PlayerDirective::FiringAt(target)
                    } else {
                        PlayerDirective::MovingToward(target)
                    }
                } else {
                    if self.directive.is_moving() {
                        // keep moving towards the target even if the mouse was released
                        self.directive
                    } else {
                        PlayerDirective::Idle
                    }
                }
            };
            self.directive = new_directive;
            Some(new_directive)
        } else {
            None
        }
    }
}

pub fn run() {

    let (input_send, input_recv) = stream_channel::<PlayerDirective>();
    let (update_send, update_recv) = channel::<GameStateUpdate>();

    thread::spawn(move || {
        let addr = "127.0.0.1:12345".parse::<SocketAddr>().unwrap();
        let mut core = Core::new().unwrap();
        let handle = core.handle();
        let tcp = TcpStream::connect(&addr, &handle).map_err(|_| ());

        let handle_tcp = tcp.and_then(|socket| {
            let io = BincodeIO::new(socket, GameClientIOMessages);
            io.read_one().map_err(|_| ()).and_then(|(msg, io)| {
                let id_result = match msg {
                    GameStateUpdate::SetClientId(id) => Ok(id),
                    msg => {
                        println!("received unexpected message from server: {:?}", msg);
                        Err(())
                    }
                };
                future::result(id_result).and_then(move |id| {
                    let (server_send, server_recv) = io.split();

                    let game_inputs = input_recv.map(move |set_directive| { GameInput::Input(PlayerInput{ from: id, set_directive })});
                    let send_inputs = server_send
                        .sink_map_err(|e| println!("Failed to send a message: {:?}", e))
                        .send_all(game_inputs)
                        .map_err(|_| println!("send channel failed"))
                        .map(|_| ());

                    let recv_updates = server_recv
                        .map_err(|_| ())
                        .for_each(move |update| update_send.send(update).map_err(|_| ()))
                        .map(|_| ());

                    send_inputs.select(recv_updates)
                        .map_err(|_| ())
                        .map(|_| ())
                })
            })
        });

        match core.run(handle_tcp.map_err(|_| ())) {
            Ok(_) => (),
            Err(_) => println!("Connection to server was dropped"),
        }
    });


    let mut window: PistonWindow = WindowSettings::new("Hello Piston!", [640, 480])
        .build()
        .unwrap();

    // values related to a border drawn near the edges of the window
    let border_margin = 10.0;
    let border = Rectangle::new_border([0., 0., 0., 1.], 0.5);

    let character_radius = 10.0;
    let character_border = Ellipse::new_border([0.5, 0.5, 0.5, 1.0], 0.5);

    // values related to mouse-based character movement
    let mut player_controls = PlayerControls::new();

    let mut updated_since_render = false;

    let mut entities = BTreeMap::<EntityId, ActorInitMemo>::new();

    while let Some(event) = window.next() {

        if let Some(new_directive) = player_controls.check_directive_change(&event) {
            input_send.unbounded_send(new_directive);
        }

        event.update(|_| {
            if !updated_since_render {
                updated_since_render = true;

                for update in update_recv.try_iter() {
                    match update {
                        GameStateUpdate::SetClientId(_) => (),
                        GameStateUpdate::Spawn(id, entity) => {
                            println!("Init entity: {:?}", entity);
                            entities.insert(id, entity);
                        },
                        GameStateUpdate::Despawn(entity_id) => {
                            println!("Despawn entity: {}", entity_id);
                            entities.remove(&entity_id);
                        }
                        GameStateUpdate::Update(id, state) => {
                            entities.get_mut(&id).unwrap().set_state(state);
                        },
                    }
                }
            }
        });

        event.render(|&args| {
            let border_dims = rectangle::rectangle_by_corners(
                border_margin,
                border_margin,
                args.width as f64 - border_margin,
                args.height as f64 - border_margin
            );

            let draw_state = DrawState::default();

            window.draw_2d(&event, |context, graphics| {

                clear([1.0; 4], graphics);
                border.draw(border_dims, &draw_state, context.transform, graphics);

                for entity in entities.values_mut() {
                    match *entity {
                        ActorInitMemo::Player(ref state, ref attributes) => {
                            let pos = state.pos;
                            let circle = Ellipse::new(attributes.color.to_vec4());
                            let circle_bounds = ellipse::circle(pos.x, pos.y, character_radius);

                            circle.draw(circle_bounds, &draw_state, context.transform, graphics);
                            character_border.draw(circle_bounds, &draw_state, context.transform, graphics);
                        },
                        ActorInitMemo::Bullet(ref state, ref attributes) => {
                            let line = Line::new_round(attributes.color.to_vec4(), 3.0);
                            let pos = state.pos;
                            let vel = attributes.vel;
                            let mut vel = Vector2::new(vel.x, vel.y);
                            vel.normalize_mut();
                            vel *= 5.0;
                            let line_coords = [pos.x - vel.x, pos.y - vel.y, pos.x + vel.x, pos.y + vel.y];
                            line.draw(line_coords, &draw_state, context.transform, graphics);
                        }
                    }
                }

            });

            updated_since_render = false;
        });

    }
}