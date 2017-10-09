// A tiny async echo server with tokio-core
extern crate bytes;
extern crate futures;
extern crate rand;
extern crate tokio_core;
extern crate tokio_io;

mod codecs;

use futures::{future, Future, Stream, Sink};
use futures::stream::{SplitSink, SplitStream};
use futures::sync::mpsc::{unbounded as stream_channel, UnboundedSender, UnboundedReceiver};

use rand::{Rand, Rng};

use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::io;
use std::net::SocketAddr;
use std::rc::Rc;
use std::str;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Sender, Receiver};
use std::thread;

use tokio_core::net::TcpListener;
use tokio_core::reactor::Core;
use tokio_io::{AsyncRead, AsyncWrite};
use tokio_io::codec::{Encoder, Decoder, Framed};


fn main() {
    let arg1 = std::env::args().nth(1);
    let arg1_ref: Option<&str> = arg1.as_ref().map(String::as_ref);

    match arg1_ref {
        Some("server") => server_main(),
        Some("client") => println!("TODO"),
        _ => println!("Argument should be either 'server' or 'client'"),
    }
}

fn server_main() {
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
        let handle_client = handshake.call(socket, peer_addr, codecs::GridTextCodec).and_then(move |(handshake_out, io)| {
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

fn fake_io_error(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::Other, msg)
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
    fn poll(&mut self) -> futures::Poll<Option<Self::Item>, Self::Error> {
        self.inner.poll()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct GridPoint(i32, i32);
impl GridPoint {
    fn move_by(&mut self, d_pos: &GridPoint) {
        self.0 += d_pos.0;
        self.1 += d_pos.1;
    }
}
impl Rand for GridPoint {
    fn rand<R: Rng>(rng: &mut R) -> Self {
        let x = (rng.next_u32() % 21) as i32 - 10;
        let y = (rng.next_u32() % 21) as i32 - 10;
        GridPoint(x, y)
    }
}

type PlayerName = char;
type PlayerUid = u64;

struct Player {
    id: PlayerUid,
    name: PlayerName,
    pos: GridPoint,
    is_newly_connected: bool,
    is_modified: bool,
}
impl Player {
    fn new(id: PlayerUid, name: PlayerName, pos: GridPoint) -> Player {
        Player {
            id,
            name,
            pos,
            is_newly_connected: true,
            is_modified: false,
        }
    }
    fn move_by(&mut self, d_pos: &GridPoint) {
        self.is_modified = true;
        self.pos.move_by(d_pos);
    }
    fn send_state_updates(&mut self, out: &UnboundedSender<GridUpdate>, replay_inits: bool) {
        if self.is_newly_connected || replay_inits {
            out.unbounded_send(GridUpdate::Connected(self.id, self.name,self.pos)).unwrap();
        }
        if self.is_modified || replay_inits {
            out.unbounded_send(GridUpdate::MovedTo(self.id, self.pos)).unwrap();
        }
        self.is_newly_connected = false;
        self.is_modified = false;
    }
}

#[derive(Debug)]
pub enum GridClientRequest {
    LoginAs(PlayerName),
    MoveRel(GridPoint),
    Unrecognized(String),
}

#[derive(Debug, Clone, Copy)]
pub enum GridClientResponse {
    LoginPrompt,
    LoggedIn(PlayerUid),
    GridUpdated(GridUpdate),
    Hangup(GridServerHangup),
}

#[derive(Debug, Clone, Copy)]
pub enum GridServerHangup {
    UnexpectedLogin,
    UnrecognizedRequest,
    InternalError
}

#[derive(Debug)]
enum GridMessage {
    Connect(PlayerUid, PlayerName, GridPoint),
    Disconnect(PlayerUid),
    MoveRel(PlayerUid, GridPoint),
}

#[derive(Debug, Clone, Copy)]
pub enum GridUpdate {
    Connected(PlayerUid, PlayerName, GridPoint),
    MovedTo(PlayerUid, GridPoint),
    Disconnected(PlayerUid),
}

struct Grid {
    players: Vec<Player>,
    incoming: Receiver<GridMessage>,
    outgoing: UnboundedSender<GridUpdate>,
}

/// Entry point for passing `GridMessages` to a `Grid`.
///
/// Messages passed in this manner are the only means of sending user input
/// to the `Grid`. Each message will be handled during the next available
/// iteration of the game loop.
#[derive(Clone)]
struct GridInbox {
    inner: Rc<Sender<GridMessage>>,
}
impl GridInbox {
    fn send(&self, msg: GridMessage) -> Result<(), ()> {
        self.inner.send(msg).map_err(|_| ())
    }
}

/// A stream of `GridUpdate` events produced by a `Grid`.
///
/// As the game loop causes things to move around and change state,
/// the `Grid` will emit events to this stream.
///
/// Because the stream has an unlimited buffer, it must be consumed
/// (i.e. polled) to avoid using too much memory.
#[must_use]
struct GridBroadcast {
    inner: UnboundedReceiver<GridUpdate>
}

impl Stream for GridBroadcast {
    type Item = GridUpdate;
    type Error = ();
    fn poll(&mut self) -> futures::Poll<Option<Self::Item>, Self::Error> {
        self.inner.poll()
    }
}

impl Grid {
    fn new() -> (Grid, GridInbox, GridBroadcast) {
        let (grid_message_sender, grid_message_receiver) = channel();
        let (grid_update_sender, grid_update_receiver) = stream_channel();
        let grid = Grid {
            players: Vec::new(),
            incoming: grid_message_receiver,
            outgoing: grid_update_sender
        };
        let grid_inbox = GridInbox { inner: Rc::new(grid_message_sender) };
        let grid_broadcast = GridBroadcast{ inner: grid_update_receiver };
        (grid, grid_inbox, grid_broadcast)
    }
    fn run(&mut self) {
        let mut event_buf: Vec<GridMessage> = Vec::new();
        loop {
            event_buf.clear();
            event_buf.extend(self.incoming.try_iter());
            for event in event_buf.iter() {
                match *event {
                    GridMessage::Connect(id, name, pos) => {
                        let player = Player::new(id, name,pos);
                        self.players.push(player);
                    },
                    GridMessage::Disconnect(id) => {
                        self.players.retain(|p| p.id != id);
                        self.outgoing.unbounded_send(GridUpdate::Disconnected(id)).unwrap();
                    },
                    GridMessage::MoveRel(id, ref d_pos) => {
                        for player in self.players.iter_mut() {
                            if player.id == id {
                                player.move_by(d_pos);
                            }
                        }
                    },
                }
            }
            for player in self.players.iter_mut() {
                player.send_state_updates(&self.outgoing, false);
            }
        };
    }
}

struct ClientHandshakeOut {
    uid: PlayerUid,
    name: PlayerName,
    addr: SocketAddr,
}

struct ClientIO<T, C> {
    writer: SplitSink<Framed<T, C>>,
    reader: SplitStream<Framed<T, C>>,
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

    fn call<T, C>(&mut self, socket: T, addr: SocketAddr, codec: C) -> Box<Future<Item = (ClientHandshakeOut, ClientIO<T, C>), Error = ()>>
        where T: AsyncRead + AsyncWrite + 'static,
        C : Decoder<Item = GridClientRequest, Error = io::Error> + Encoder<Item = GridClientResponse, Error = io::Error> + 'static
    {
        let (writer, reader) = socket.framed(codec).split();
        let uid_counter = self.uid_counter.clone();
        // Send a login prompt
        // This gives a future containing the "continuation" of the writer.
        let raw_future = writer.send(GridClientResponse::LoginPrompt).and_then(move |writer_cont| {

            // Read a message from the client, which is hopefully a Login.
            // This gives a future containing the message and the "continuation" of the read stream.
            reader.into_future().map_err(|(err, _)| err).and_then(move |(hopefully_login, read_cont)| {

                // Check that the received message was a login attempt, treating anything else as an error.
                let login_attempt: io::Result<PlayerName> = match hopefully_login {
                    Some(GridClientRequest::LoginAs(player_name)) => Ok(player_name),
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
                    writer_cont.send(GridClientResponse::LoggedIn(uid)).and_then(move |writer_cont| {
                        let handshake_result = ClientHandshakeOut { uid, name, addr };
                        let io = ClientIO { writer: writer_cont, reader: read_cont };
                        future::ok((handshake_result, io))
                    })
                })
            })
        }).map_err(|_| ());

        Box::new(raw_future)
    }
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


/// Generalization of a service that handles clients in four separate steps:
///
///  - **Connect** takes some "handshake" data and an IO object, performing any necessary side-effects.
///  - **Handle Input** treats the client's input as a Stream, yielding a Future that completes when all input has been handled.
///  - **Handle Output** treats the client's output as a Sink, returning a Future that completes when all output has been sent.
///  - **Disconnect** performs some side-effects when either the Input or Output handler finishes or fails
///
trait IOService<T, C> where Self : Clone + 'static {
    type HandshakeResult;
    type ClientType : Clone + 'static;
    type ClientInputData;
    type ClientOutputData;
    type UnitFuture : Future<Item = (), Error = ()> + 'static;
    type ConnectionFuture : Future<Item = (Self::ClientType, Self::ClientInputData, Self::ClientOutputData, ClientIO<T, C>), Error = ()> + 'static;

    fn on_connect(&self, handshake_result: Self::HandshakeResult, io: ClientIO<T, C>) -> Self::ConnectionFuture;
    fn handle_incoming(&self, client: Self::ClientType, client_input_data: Self::ClientInputData, input: SplitStream<Framed<T, C>>) -> Self::UnitFuture;
    fn handle_outgoing(&self, client: Self::ClientType, client_output_data: Self::ClientOutputData, output: SplitSink<Framed<T, C>>) -> Self::UnitFuture;
    fn handle_disconnect(&self, client: Self::ClientType) -> Self::UnitFuture;

    /// Main handler method.
    ///
    /// Given the result of an already-completed handshake and an IO object,
    /// run the connection handler to get the associated input/output data,
    /// then run the input/output handlers with their respective data and halves of the IO object,
    /// finally running the disconnect handler.
    ///
    fn handle_client(self, handshake_result: Self::HandshakeResult, io: ClientIO<T, C>) -> Box<Future<Item = (), Error = ()>>
    {
        let raw_future = self.on_connect(handshake_result, io).and_then(move |(client, client_input_data, client_output_data, io)| {
            // Grab the R/W & I/O objects from the connect result.
            // Note that the method's `io` parameter is consumed by `on_connect`, since that method could potentially do some IO.
            let ClientIO{ reader, writer } = io;

            // Run the input and output handlers to get a Future for each respective completion/hangup.
            // Clone the `client_ident` because the handler implementations will want to move the value, and we need to be able to use it later.
            let handled_input = self.handle_incoming(client.clone(), client_input_data, reader);
            let handled_output = self.handle_outgoing(client.clone(), client_output_data, writer);

            // Select the first of the IO futures to either finish or fail.
            // We don't care about the return type, as it will be discarded by the disconnect handler.
            // We do need to make sure the error type stays as (), so we need to map it from the Select combinator's special error type.
            let handled_io = handled_input.select(handled_output)
                .map_err(|(_first_err, _next_future)| ());

            // Once the IO is done, run the disconnect handler.
            // IMPORTANT: use `then`, not `and_then`, because we want this closure to run regardless of the success or error of the IO handler.
            // If the client goofs up and we hang up on them, that's an error, but we still want to de-register that client!
            handled_io.then(move |_| {
                self.handle_disconnect(client)
            })
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

impl <T, C> IOService<T, C> for GridServerHandle
    where T: AsyncRead + AsyncWrite + 'static,
          C: Decoder<Item = GridClientRequest, Error = io::Error> + Encoder<Item = GridClientResponse, Error = io::Error> + 'static
{
    type HandshakeResult = ClientHandshakeOut;
    type ClientType = Client;
    type ClientInputData = ();
    type ClientOutputData = ClientBroadcast;
    type UnitFuture = Box<Future<Item = (), Error = ()>>;
    type ConnectionFuture = Box<Future<Item = (Self::ClientType, Self::ClientInputData, Self::ClientOutputData, ClientIO<T, C>), Error = ()>>;

    /// When a client connects, add it to the "client registry" so that future GridUpdates can be broadcast to it.
    /// Notify the grid of a "new player" associated with the new client.
    ///
    fn on_connect(&self, handshake_result: ClientHandshakeOut, io: ClientIO<T, C>) -> Self::ConnectionFuture {
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
    fn handle_incoming(&self, client_ident: Client, _client_input_data: (), input: SplitStream<Framed<T, C>>) -> Self::UnitFuture {
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
    fn handle_outgoing(&self, _client: Client, client_output_data: ClientBroadcast, output: SplitSink<Framed<T, C>>) -> Self::UnitFuture {
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
        if removed.is_some() {
            println!("successfully de-registered the disconnected client");
        } else {
            println!("failed to de-register the disconnected client :(");
        }
        let notified_grid_result = self.grid_inbox.send(GridMessage::Disconnect(uid));
        Box::new(future::result(notified_grid_result))
    }

}