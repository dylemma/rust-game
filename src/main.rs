// A tiny async echo server with tokio-core
extern crate bytes;
extern crate futures;
extern crate rand;
extern crate tokio_core;
extern crate tokio_io;

mod game;
mod server;

fn main() {
    let arg1 = std::env::args().nth(1);
    let arg1_ref: Option<&str> = arg1.as_ref().map(String::as_ref);

    match arg1_ref {
        Some("server") => server::run(),
        Some("client") => println!("TODO"),
        _ => println!("Argument should be either 'server' or 'client'"),
    }
}
