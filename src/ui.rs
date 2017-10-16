use piston_window::*;
use graphics::ellipse;
use na::{Vector2};
use fps_counter::FPSCounter;
use std::time::Instant;

pub fn run() {
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

    while let Some(event) = window.next() {

        // track mouse position
        event.mouse_cursor(|x, y| {
            mouse_pos[0] = x;
            mouse_pos[1] = y;
            if is_mouse_down {
                character_target[0] = x;
                character_target[1] = y;
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
                let dt = args.dt;
                let dist = dt * follow_speed;
                let mut movement: Vector2<f64> = Vector2::from(character_target) - Vector2::from(character_pos);
                if movement.try_normalize_mut(1.0).is_some() {
                    movement *= dist;
                    character_pos[0] += movement.x;
                    character_pos[1] += movement.y;
                }

                let fps = fps_counter.tick();
                println!("{} fps", fps);
            }
        });

        event.render(|&args| {
            let border_dims = rectangle::rectangle_by_corners(
                border_margin,
                border_margin,
                args.width as f64 - border_margin,
                args.height as f64 - border_margin
            );
            let character_dims = ellipse::circle(character_pos[0], character_pos[1], character_radius);
            let draw_state = DrawState::default();

            window.draw_2d(&event, |context, graphics| {
                clear([1.0; 4], graphics);
                border.draw(border_dims, &draw_state, context.transform, graphics);
                character.draw(character_dims, &draw_state, context.transform, graphics);
                character_border.draw(character_dims, &draw_state, context.transform, graphics);
            });

            updated_since_render = false;
        });

    }
}