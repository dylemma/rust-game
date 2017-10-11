use server::io::*;

use binio::*;

use futures::{Future};

use std::io;
use std::net::SocketAddr;

use tokio_core::net::TcpStream;
use tokio_core::reactor::Core;

struct ClientIOMessages;
impl IOMessages for ClientIOMessages {
    type Input = GridClientResponse;
    type Output = GridClientRequest;
}

fn io_error(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::Other, msg)
}

fn read_nonempty_line() -> io::Result<String> {
    let mut input = String::new();
    io::stdin().read_line(&mut input).and_then(move |num_read| {
        if num_read == 0 {
            Err(io_error("reached end of STDIN while trying to read a line"))
        } else {
            input.trim_right();
            if input.is_empty() {
                read_nonempty_line()
            } else {
                Ok(input)
            }
        }
    })
}

pub fn run() {
    let addr = "127.0.0.1:12345".parse::<SocketAddr>().unwrap();
    let mut core = Core::new().unwrap();
    let handle = core.handle();
    let tcp = TcpStream::connect(&addr, &handle);

    let handshake = tcp.and_then(|stream| {
        BincodeIO::new(stream, ClientIOMessages)
            // we're expecing a Login Prompt message
            .read_one().and_then(|(msg, io)| {
                match msg {
                    GridClientResponse::LoginPrompt => Ok(io),
                    _ => Err(io_error("expected login prompt"))
                }
            })
            // now that we got the prompt from the server, prompt the user for a name
            .and_then(|io|{
                // TODO: restore prompt logic
//                println!("Enter your name to log in! (really just one letter for now)");
//                read_nonempty_line().map(|line| (line.chars().nth(0).unwrap(), io))
                Ok(('D', io))
            })
            // send the name to the server as a `LoginAs` request
            .and_then(|(player_name, io)| {
                // we want the player_name for later, so we'll start nesting futures
                io
                    .write_one(GridClientRequest::LoginAs(player_name))
                    // expect a `LoggedIn` response from the server, containing our `player_id`
                    .and_then(BincodeIO::read_one).and_then(|(msg, io)| {
                        match msg {
                            GridClientResponse::LoggedIn(player_id) => Ok((player_id, io)),
                            _ => Err(io_error("Expected a login acknowledgement")),
                        }
                    })
                    .and_then(move |(player_id, _io)| {
                        println!("Logged in as {} with id {}, but I'm out of things to do, so I'm disconnecting!", player_name, player_id);
                        Ok(())
                    })
            })
    });

    core.run(handshake).unwrap();
}