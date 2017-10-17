// A tiny async echo server with tokio-core
extern crate bincode;
extern crate bytes;
extern crate futures;
extern crate rand;
extern crate serde;
#[macro_use] extern crate serde_derive;
extern crate tokio_core;
extern crate tokio_io;
extern crate tokio_proto;
extern crate tokio_serde_bincode;
extern crate tokio_timer;

extern crate piston;
extern crate piston_window;
extern crate graphics;
extern crate opengl_graphics;
extern crate input;
extern crate nalgebra as na;

mod game;
mod server;
mod ui;
mod binio;

fn main() {
    let arg1 = std::env::args().nth(1);
    let arg1_ref: Option<&str> = arg1.as_ref().map(String::as_ref);

    match arg1_ref {
        Some("server") => server::run(),
        Some("ui") => ui::run(),
        Some("game") => server::run_game(),
        _ => println!("Argument should be either 'server' or 'client'"),
    }
}
