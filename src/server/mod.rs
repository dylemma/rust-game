mod proto;
use self::proto::*;

pub mod io;
use self::io::*;

use futures::{future, Future, Poll, Stream, Sink};
use futures::sync::mpsc::{unbounded as stream_channel, UnboundedSender, UnboundedReceiver};

use game::*;
use binio::*;

use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::io::{Error as IOError, ErrorKind as IOErrorKind, Result as IOResult};
use std::net::SocketAddr;
use std::rc::Rc;
use std::str;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};
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

pub fn run() {
    // The event loop that deals with all of the async IO we want to do
    let mut core = Core::new().unwrap();
    let handle = core.handle();

    // Set up a TCP listener that will accept new connections
    let address = "0.0.0.0:12345".parse().unwrap();
    let listener = TcpListener::bind(&address, &core.handle()).unwrap();

    // Create a new `Grid` (a.k.a. the game world).
    // This gets us:
    //  - the actual `Grid`
    //  - a handle to send messages to the Grid (like user inputs)
    //  - a stream of game state updates which needs to be read
    let (mut grid, grid_inbox,grid_broadcast) = Grid::new();

    // Run the game loop in a dedicated thread
    thread::spawn(move || grid.run());

    // Whenever the game updates, we want to tell all connected clients about it.
    // This is a collection of client handles which can be used to do just that.
    // The server will add and remove entries from this registry as clients
    // connect and disconnect.
    let client_registry = ClientRegistry::new();

    // A lightweight reference to the game state.
    // This also acts as the client handler thanks to its IOService implementation.
    // Each client that connects will be handled by this (or a clone of it).
    let server_handle = GridServerHandle {
        clients: client_registry.clone(),
        grid_inbox: grid_inbox.clone(),
    };

    // A struct representing the handshake process that each client must go through
    // in order to really connect and start interacting with the game.
    let mut handshake = ClientHandshake::new();

    // A Future which pulls messages from the `grid_broadcast` and sends them
    // to all connected clients.
    //
    // This Future should never complete on its own, barring a `panic!`
    //
    // NOTE: this doesn't actually do anything when we declare it.
    // It needs to be passed along to `core.run` (which we do indirectly by first
    // combining it with another Future and *then* passing it).
    let consume_grid_broadcast = {
        let client_registry = client_registry.clone();
        grid_broadcast.for_each(move |update| {
            println!("Broadcast update: {:?}", update);
            // send the update to all of the clients
            client_registry.broadcast(GridClientResponse::GridUpdated(update));
            future::ok(())
        })
    };

    // A Future which handles all incoming TCP connections on our particular port.
    // For each new connection, it runs the handshake and, if successfull, spawns
    // a new handler task (which runs on the `core` event loop) to manage the connection.
    //
    // This Future should never complete on its own, barring a `panic!`
    let server = listener.incoming().for_each(|(socket, peer_addr)| {

        // clone the server handle to be captured in the following closure
        let server_handle = server_handle.clone();

        // run the handshake and pass its result along to the `server_handle` to get a Future representing the completed handling of the client
        let handle_client = handshake.call(socket, peer_addr).and_then(move |(handshake_out, io)| {
            server_handle.handle_client(handshake_out, io)
        });

        // now run the client handler in the event loop
        handle.spawn(handle_client);
        Ok(())
    });

    // Combination of the grid_broadcast and connection handlers as a single Future which completes
    // when either of the two either complete or fail.
    let server_and_broadcast = consume_grid_broadcast.map_err(|_| fake_io_error("uh oh")).select(server);

    // Actually run the two futures defined above.
    // This will block forever, ideally, since the futures are designed not to complete.
    // If it does stop, it's effectively a server crash.
    let x = core.run(server_and_broadcast);
    match x {
        Ok(_) => (),
        Err(_e) => println!("Server crashed!"),
    }
}

fn fake_io_error(msg: &str) -> IOError {
    IOError::new(IOErrorKind::Other, msg)
}

#[derive(Clone)]
struct Client {
    uid: PlayerUid,
    name: PlayerName,
    addr: SocketAddr,
    inbox: ClientInbox
}
impl Client {
    fn new(uid: PlayerUid, name: PlayerName, addr: SocketAddr) -> (Client, ClientBroadcast) {
        let (response_sender, response_receiver) = stream_channel::<GridClientResponse>();
        let inbox = ClientInbox { inner: Rc::new(response_sender) };
        let client = Client { uid, name, addr, inbox };
        let broadcast = ClientBroadcast { inner: response_receiver };
        (client, broadcast)
    }
}

///
#[derive(Clone)]
struct ClientInbox {
    inner: Rc<UnboundedSender<GridClientResponse>>
}
impl ClientInbox {
    fn push(&self, msg: GridClientResponse) {
        self.inner.unbounded_send(msg).unwrap();
    }
}

/// A stream of responses that must be sent to a Client's underlying socket.
///
/// Any messages sent to the corresponding `ClientInbox` will arrive here.
/// The stream *must* be consumed, or else the buffer will grow indefinitely.
#[must_use]
struct ClientBroadcast {
    inner: UnboundedReceiver<GridClientResponse>
}
impl Stream for ClientBroadcast {
    type Item = GridClientResponse;
    type Error = ();
    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        self.inner.poll()
    }
}





struct ClientHandshakeOut {
    uid: PlayerUid,
    name: PlayerName,
    addr: SocketAddr,
}


#[derive(Clone)]
struct ClientRegistry {
    inner: Arc<Mutex<HashMap<PlayerUid, ClientInbox, RandomState>>>,
}
impl ClientRegistry {
    fn new() -> Self {
        ClientRegistry { inner: Arc::new(Mutex::new(HashMap::new())) }
    }
    fn register(&self, client: &Client) {
        let uid = client.uid;
        let mut clients = self.inner.lock().unwrap();
        clients.insert(uid,client.inbox.clone());
    }
    fn remove(&self, uid: &PlayerUid) -> Option<ClientInbox> {
        let mut clients = self.inner.lock().unwrap();
        clients.remove(&uid)
    }
    fn broadcast(&self, msg: GridClientResponse) {
        let clients = self.inner.lock().unwrap();
        for inbox in clients.values() {
            inbox.push(msg);
        }
    }
}

pub type GameIO<T> = BincodeIO<T, GameInput, GameStateUpdate>;
pub type GameIn<T> = BincodeStream<T, GameInput>;
pub type GameOut<T> = BincodeSink<T, GameStateUpdate>;

pub struct GameIOMessages;
impl IOMessages for GameIOMessages {
    type Input = GameInput;
    type Output = GameStateUpdate;
}

pub type ServerIO<T> = BincodeIO<T, GridClientRequest, GridClientResponse>;
pub type ServerIn<T> = BincodeStream<T, GridClientRequest>;
pub type ServerOut<T> = BincodeSink<T, GridClientResponse>;

struct GameServer {
    handle: Rc<GameHandle>,
}
impl GameServer {
    fn handle_client<T>(&self, handle: &Handle, socket: T, addr: SocketAddr) -> ()
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
                    .map_err(|e| ());

                // Read all messages from the client from the socket, and pass them along to the game.
                // An error reading from the socket means the client disconnected.
                // An error sending to the game means the game has shut down.
                let game_handle_for_recv = game_handle.clone();
                let handle_recv = {
                    socket_recv
                        .map_err(|e| ())
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

struct ServerIOMessages;
impl IOMessages for ServerIOMessages {
    type Input = GridClientRequest;
    type Output = GridClientResponse;
}

struct ClientHandshake {
    uid_counter: Rc<AtomicUsize>,
}

impl ClientHandshake {
    fn new() -> ClientHandshake {
        ClientHandshake {
            uid_counter: Rc::new(AtomicUsize::new(0)),
        }
    }

    fn call<T>(&mut self, socket: T, addr: SocketAddr) -> Box<Future<Item = (ClientHandshakeOut, ServerIO<T>), Error = ()>>
        where T: AsyncRead + AsyncWrite + 'static
    {
        let io = BincodeIO::new(socket, ServerIOMessages);
        let uid_counter = self.uid_counter.clone();
        // Send a login prompt
        // This gives a future containing the "continuation" of the writer.
        let raw_future = io.write_one(GridClientResponse::LoginPrompt).and_then(move |io| {

            // Read a message from the client, which is hopefully a Login.
            // This gives a future containing the message and the "continuation" of the read stream.
            io.read_one().and_then(move |(hopefully_login, io)| {

                // Check that the received message was a login attempt, treating anything else as an error.
                let login_attempt: IOResult<PlayerName> = match hopefully_login {
                    GridClientRequest::LoginAs(player_name) => Ok(player_name),
                    other => {
                        println!("Unsuccessful login attempt from {}: {:?}", addr, other);
                        Err(fake_io_error("Expected login attempt"))
                    }
                };

                // Assuming the login was Ok, generate a PlayerUid for the client...
                future::result(login_attempt).and_then(move |name| {
                    let uid: u64 = uid_counter.fetch_add(1, Ordering::SeqCst) as u64;
                    println!("Successful login handshake by client {} with name {}", uid, name);

                    // Send the client an acknowledgement containing the PlayerUid
                    io.write_one(GridClientResponse::LoggedIn(uid)).and_then(move |io| {
                        let handshake_result = ClientHandshakeOut { uid, name, addr };
                        future::ok((handshake_result, io))
                    })
                })
            })
        }).map_err(|err| {
            println!("IO error during client handshake: {:?}", err);
        });

        Box::new(raw_future)
    }
}


/// IOService that communicates with `GridClient` Request/Response messages to
/// drive updates to a `Grid` and broadcast updates to all connected clients.
#[derive(Clone)]
struct GridServerHandle {
    clients: ClientRegistry,
    grid_inbox: GridInbox,
}

impl <T> IOService<T> for GridServerHandle
    where T: AsyncRead + AsyncWrite + 'static
{
    type HandshakeResult = ClientHandshakeOut;
    type ClientType = Client;
    type ClientInputData = ();
    type ClientOutputData = ClientBroadcast;
    type UnitFuture = Box<Future<Item = (), Error = ()>>;
    type ConnectionFuture = Box<Future<Item = (Self::ClientType, Self::ClientInputData, Self::ClientOutputData, ServerIO<T>), Error = ()>>;

    /// When a client connects, add it to the "client registry" so that future GridUpdates can be broadcast to it.
    /// Notify the grid of a "new player" associated with the new client.
    ///
    fn on_connect(&self, handshake_result: ClientHandshakeOut, io: ServerIO<T>) -> Self::ConnectionFuture {
        let ClientHandshakeOut{ uid, name, addr } = handshake_result;
        let (client, client_broadcast) = Client::new(uid, name, addr);

        // Register the client in the "connections" map.
        self.clients.register(&client);

        // Request a new "Player" be added to the grid at point {0, 0}
        self.grid_inbox.send(GridMessage::Connect(uid, name, GridPoint(0, 0))).unwrap();

        Box::new(future::ok((client, (), client_broadcast, io)))
    }

    /// Handle all of the requests made by the client.
    ///
    /// Certain events are treated as errors which cause the server to hang up on the client.
    /// The rest are interpreted as `GridMessages` and forwarded to the `Grid`.
    ///
    fn handle_incoming(&self, client_ident: Client, _client_input_data: (), input: ServerIn<T>) -> Self::UnitFuture {
        let client = client_ident;
        let uid = client.uid;
        let client_inbox = client.inbox.clone();
        let grid_inbox = self.grid_inbox.clone();

        let raw_handler = input.for_each(move |msg| {
            println!("Received message from {} - {:?}", uid, msg);
            let action: Result<GridMessage, GridServerHangup> = match msg {
                GridClientRequest::LoginAs(_) => Err(GridServerHangup::UnexpectedLogin),
                GridClientRequest::Unrecognized(_) => Err(GridServerHangup::UnrecognizedRequest),
                GridClientRequest::MoveRel(d_pos) => Ok(GridMessage::MoveRel(uid, d_pos)),
            };

            match action {
                Ok(grid_msg) => {
                    let send_result = grid_inbox.send(grid_msg).map_err(|_| {
                        client_inbox.push(GridClientResponse::Hangup(GridServerHangup::InternalError));
                        fake_io_error("Hangup after failure to send grid message")
                    });
                    future::result(send_result)
                },
                Err(hangup) => {
                    println!("About to hang up on client {} because: {:?}", uid, hangup);
                    client_inbox.push(GridClientResponse::Hangup(hangup));
                    future::err(fake_io_error("Hangup on misbehaving client"))
                }
            }
        }).map_err(|_| ());

        Box::new(raw_handler)
    }

    /// Send all of the response messages to the client through the socket.
    ///
    /// Most of the messages sent in this manner are coming from the `Grid`'s game loop,
    /// which broadcasts its state updates to all connected clients.
    fn handle_outgoing(&self, _client: Client, client_output_data: ClientBroadcast, output: ServerOut<T>) -> Self::UnitFuture {
        let client_broadcast = client_output_data;
        let outgoing_messages = client_broadcast.map_err(|_| fake_io_error("Error receiving broadcast message"));
        let raw_future = output.send_all(outgoing_messages)
            .map(|_| ())
            .map_err(|_| ());

        Box::new(raw_future)
    }

    /// De-register the client so that the server doesn't keep trying to broadcast messages to it.
    ///
    fn handle_disconnect(&self, client: Client) -> Self::UnitFuture {
        let uid = client.uid;
        let removed = self.clients.remove(&uid);
        if removed.is_none() {
            panic!("failed to de-register a disconnecting client")
        }
        let notified_grid_result = self.grid_inbox.send(GridMessage::Disconnect(uid));
        Box::new(future::result(notified_grid_result))
    }

}