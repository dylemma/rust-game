use server::io::*;

use binio::*;
use game::*;

use futures::future;
use futures::{Future, Sink, Stream};
use futures::sync::mpsc;

use std::io::{self, BufRead};
use std::net::SocketAddr;
use std::thread;

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

/// Turn STDIN into a Stream that can be polled and used effectively by other Futures/Streams.
///
/// Taken from the example at https://tokio.rs/docs/going-deeper-futures/synchronization/#channels
///
/// The downside of this approach is that when the returned reference is finally dropped,
/// one more line must be consumed and thrown away by the Err => break case.
/// So don't drop the reference!
///
/// NOTE:
/// It's important that we don't try to read from STDIN from the context of a Future or Stream.
/// It messes with the polling mechanisms and manifests as messages being sent out with weird
/// delays, or messages never being received. Just don't mess around with blocking IO inside
/// event-based loop constructs.
fn stdin_lines() -> Box<Stream<Item = String, Error = io::Error>> {
    let (mut tx, rx) = mpsc::channel(0);

    thread::spawn(|| {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            match tx.send(line).wait() {
                Ok(t) => tx = t,
                Err(_) => {
                    println!("stdin_lines stream ending due to send error. The previous line was lost.");
                    break
                },
            }
        }
    });

    Box::new(rx.then(|r| r.unwrap()))
}

pub fn run() {
    let addr = "127.0.0.1:12345".parse::<SocketAddr>().unwrap();
    let mut core = Core::new().unwrap();
    let handle = core.handle();
    let tcp = TcpStream::connect(&addr, &handle);

    let stdin = stdin_lines();

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
            .and_then(move |io|{
                println!("Enter your name to log in! (really just one letter for now)");
                stdin.into_future().map_err(|(e, _stdin)| e).and_then(|(line, stdin)| {
                    let player_name_result = match line {
                        Some(line) => {
                            match line.chars().nth(0) {
                                Some(c) => Ok(c),
                                None => Err(io_error("blank name for login")),
                            }
                        },
                        None => Err(io_error("expected a name for login")),
                    };

                    future::result(player_name_result).and_then(move |player_name| {
                        io
                            .write_one(GridClientRequest::LoginAs(player_name))
                            // expect a `LoggedIn` response from the server, containing our `player_id`
                            .and_then(BincodeIO::read_one).and_then(|(msg, io)| {
                                match msg {
                                    GridClientResponse::LoggedIn(player_id) => Ok((player_id, io)),
                                    _ => Err(io_error("Expected a login acknowledgement")),
                                }
                            })
                            .and_then(move |(player_id, io)| {
                                println!("Logged in as {} with id {}, but I'm out of things to do, so I'm disconnecting!", player_name, player_id);
                                Ok((player_id, player_name, io, stdin))
                            })
                    })
                })
            })
    });

    let run_client = {
        handshake.and_then(|(player_id, player_name, io, stdin)| {
            println!("Connected as {} with ID {}. Start typing stuff!", player_name, player_id);

            let (writer, reader) = io.split();

            let receive_responses = reader.for_each(|response| {
                println!("Got response from server: {:?}", response);
                Ok(())
            });

            let requests = stdin.filter_map(|line| {
                match line.as_ref() {
                    "up" => Some(GridClientRequest::MoveRel(GridPoint(0, -1))),
                    "down" => Some(GridClientRequest::MoveRel(GridPoint(0, 1))),
                    "left" => Some(GridClientRequest::MoveRel(GridPoint(-1, 0))),
                    "right" => Some(GridClientRequest::MoveRel(GridPoint(0, 1))),
                    s => {
                        println!("You entered {:?} but IDK what to do with that", s);
                        None
                    }
                }
            });

            let send_requests = writer.send_all(requests).map(|_| ());

            receive_responses.select(send_requests)
                .map_err(|(e, _)| e)
                .map(|_| ())
        })
    };

    match core.run(run_client) {
        Ok(()) => println!("Client code finished"),
        Err(e) => println!("Client finished with error: {:?}", e),
    }
}