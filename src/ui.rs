use piston_window::*;
use graphics::ellipse;
use na::{Vector2};
use fps_counter::FPSCounter;
use std::time::Instant;
use std::sync::mpsc::channel;
use binio::*;
use game;
use game::*;
use tokio_core::net::TcpStream;
use tokio_core::reactor::Core;
use std::net::SocketAddr;
use std::thread;
use futures::future;
use futures::{Future, Sink, Stream};
use futures::sync::mpsc::{unbounded as stream_channel};
use std::collections::BTreeMap;

struct GameClientIOMessages;
impl IOMessages for GameClientIOMessages {
    type Input = GameStateUpdate;
    type Output = GameInput;
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

    // let chara
    let character_radius = 10.0;
    let character = Ellipse::new([0.0, 0.8, 0.3, 1.0]);
    let character_border = Ellipse::new_border([0.5, 0.5, 0.5, 1.0], 0.5);

    // values related to mouse-based character movement
    let mut character_pos = [100.0, 100.0];
    let mut character_target = [100.0, 100.0];
    let mut mouse_pos = [0.0, 0.0];
    let mut is_mouse_down = false;
    let follow_speed = 300.0;

    let mut fps_counter = FPSCounter::new();
    let mut updated_since_render = false;

    let mut entities = BTreeMap::<EntityId, Entity>::new();

    while let Some(event) = window.next() {

        // track mouse position
        event.mouse_cursor(|x, y| {
            mouse_pos[0] = x;
            mouse_pos[1] = y;
            if is_mouse_down {
                character_target[0] = x;
                character_target[1] = y;
                let directive = PlayerDirective::MovingToward(Target::Location(Vec2::new(x, y)));
                input_send.unbounded_send(directive);
            }
        });

        // track mouse left-click state
        event.button(|btn| {
            match btn.button {
                Button::Mouse(MouseButton::Left) => {
                    match btn.state {
                        ButtonState::Press => {
                            is_mouse_down = true;
                            character_target[0] = mouse_pos[0];
                            character_target[1] = mouse_pos[1];
                            let directive = PlayerDirective::MovingToward(Target::Location(Vec2::new(mouse_pos[0], mouse_pos[1])));
                            input_send.unbounded_send(directive);
                        },
                        ButtonState::Release => is_mouse_down = false
                    }
                },
                _ => ()
            }
        });

        event.update(|&args| {
            if !updated_since_render {
                updated_since_render = true;

                for update in update_recv.try_iter() {
                    match update {
                        GameStateUpdate::SetClientId(_) => (),
                        GameStateUpdate::Init(entity) => {
                            println!("Init entity: {:?}", entity);
                            entities.insert(entity.id, entity);
                        },
                        GameStateUpdate::Update(id, state) => {
                            entities.get(&id).unwrap().data.set_state(state);
                        },
                    }
                }


//                let dt = args.dt;
//                let dist = dt * follow_speed;
//                let mut movement: Vector2<f64> = Vector2::from(character_target) - Vector2::from(character_pos);
//                if movement.try_normalize_mut(1.0).is_some() {
//                    movement *= dist;
//                    character_pos[0] += movement.x;
//                    character_pos[1] += movement.y;
//                }

//                let fps = fps_counter.tick();
//                println!("{} fps", fps);
            }
        });

        event.render(|&args| {
            let border_dims = rectangle::rectangle_by_corners(
                border_margin,
                border_margin,
                args.width as f64 - border_margin,
                args.height as f64 - border_margin
            );

//            let character_dims = ellipse::circle(character_pos[0], character_pos[1], character_radius);
            let draw_state = DrawState::default();

            window.draw_2d(&event, |context, graphics| {

                clear([1.0; 4], graphics);
                border.draw(border_dims, &draw_state, context.transform, graphics);

                for entity in entities.values() {
                    match entity.data {
                        EntityAttribs::Player{ ref attributes, ref state } => {
                            let state = state.borrow();
                            let pos = state.pos;
                            let circle = Ellipse::new(attributes.color.to_vec4());
                            let circle_bounds = ellipse::circle(pos.x, pos.y, character_radius);

                            circle.draw(circle_bounds, &draw_state, context.transform, graphics);
                            character_border.draw(circle_bounds, &draw_state, context.transform, graphics);
                        },
                        EntityAttribs::Bullet { ref attributes, ref state } => (),
                    }
                }

            });

            updated_since_render = false;
        });

    }
}