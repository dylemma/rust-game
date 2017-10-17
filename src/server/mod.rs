use futures::{future, Future, Stream, Sink};
use futures::sync::mpsc::{unbounded as stream_channel};

use game::*;
use binio::*;

use std::io::{Error as IOError, ErrorKind as IOErrorKind};
use std::net::SocketAddr;
use std::rc::Rc;
use std::str;
use std::sync::mpsc::SendError;
use std::thread;

use tokio_core::net::TcpListener;
use tokio_core::reactor::{Core, Handle};
use tokio_io::{AsyncRead, AsyncWrite};

pub fn run_game() {
    use game::*;
    let mut core = Core::new().unwrap();
    let handle = core.handle();

    let (mut game, game_handle) = Game::new();
    thread::spawn(move || game.run());

    let server = GameServer {
        handle: Rc::new(game_handle)
    };

    // Set up a TCP listener that will accept new connections
    let address = "0.0.0.0:12345".parse().unwrap();
    let listener = TcpListener::bind(&address, &core.handle()).unwrap();

    let run_server = listener.incoming().for_each(move |(socket, peer_addr)| {
        Ok(server.handle_client(&handle, socket, peer_addr))
    });

    match core.run(run_server) {
        Ok(_) => (),
        Err(_) => println!("Server crashed!"),
    }
}

fn fake_io_error(msg: &str) -> IOError {
    IOError::new(IOErrorKind::Other, msg)
}

pub struct GameIOMessages;
impl IOMessages for GameIOMessages {
    type Input = GameInput;
    type Output = GameStateUpdate;
}

struct GameServer {
    handle: Rc<GameHandle>,
}
impl GameServer {
    fn handle_client<T>(&self, handle: &Handle, socket: T, _addr: SocketAddr) -> ()
        where T: AsyncRead + AsyncWrite + 'static
    {
        // channel for sending messages from the Game to the Client
        let (client_send, client_recv) = stream_channel();
        let game_handle = self.handle.clone();

        let io = BincodeIO::new(socket, GameIOMessages);

        let client_id_future: Box<Future<Item = EntityId, Error = ()>> = Box::new(
            future::result(self.handle.new_connection(client_send))
            .map_err(|_| {
                println!("Failed to notify server of new connection. Game must be shutting down");
            })
            .and_then(|receiver| {
                receiver.map_err(|_| println!("Game cancelled receipt of new connection"))
            })
        );

        let handler = client_id_future.and_then(|client_id| {
            io.write_one(GameStateUpdate::SetClientId(client_id)).map_err(|_| ()).and_then(move |io| {
                let (socket_send, socket_recv) = io.split();

                let state_updates_broadcast = client_recv
                    .map_err(|_| {
                        // upgrade error type to io::Error so we can use send_all
                        fake_io_error("Error in UnboundedReceiver stream.")
                    });

                // Send all of the StateUpdates to the client over the socket.
                // An error here means the client disconnected.
                let handle_send = socket_send.send_all(state_updates_broadcast)
                    .map(|_| ())
                    .map_err(|_| ());

                // Read all messages from the client from the socket, and pass them along to the game.
                // An error reading from the socket means the client disconnected.
                // An error sending to the game means the game has shut down.
                let game_handle_for_recv = game_handle.clone();
                let handle_recv = {
                    socket_recv
                        .map_err(|_| ())
                        .for_each(move |game_input| {
                            game_handle_for_recv.send(game_input).map_err(move |SendError(unsent_input)| {
                                println!("Failed to send {:?} to the game from client {}. Game must be shutting down", unsent_input, client_id);
                            })
                        })
                };

                // Get the first completed (success or failure)
                let game_handle_for_finish = game_handle.clone();
                handle_send.select(handle_recv)
                    .map(|_| ())
                    .map_err(|_| ())
                    .then(move |_| {
                        game_handle_for_finish.dropped_connection(client_id).map_err(move |_| {
                            println!("Failed to notify server of client {}'s disconnection. Game must be shutting down", client_id);
                        })
                    })
            })
        });

        handle.spawn(handler);
    }
}
