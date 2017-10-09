// A tiny async echo server with tokio-core
extern crate bytes;
extern crate futures;
extern crate rand;
extern crate tokio_core;
extern crate tokio_io;
extern crate tokio_proto;
extern crate tokio_service;

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
use tokio_proto::pipeline::ServerProto;
use tokio_proto::TcpServer;
use tokio_service::Service;


fn main() {
    let arg1 = std::env::args().nth(1);
    let arg1_ref: Option<&str> = arg1.as_ref().map(String::as_ref);

    match arg1_ref {
        Some("server") => {
            let addr = "127.0.0.1:12345".parse().unwrap();
            println!("Running server on {:?}", addr);
            let server = TcpServer::new(LineProto, addr);
            server.serve(|| Ok(EchoReverse));
        },
        Some("server2") => {
            let mut core = Core::new().unwrap();
            let handle = core.handle();
            let address = "0.0.0.0:12345".parse().unwrap();
            let listener = TcpListener::bind(&address, &core.handle()).unwrap();
            let (mut grid, grid_inbox,grid_broadcast) = Grid::new();
            thread::spawn(move || grid.run());

            let client_registry = ClientRegistry::new();

            let server_stuff = ServerStuff {
                clients: client_registry.clone(),
                grid_inbox: grid_inbox.clone(),
            };

            let mut handshake = ClientHandshake::new();


            let client_registry_capture1 = client_registry.clone();
            let consume_grid_broadcast =
                grid_broadcast.for_each(move |update| {
                    println!("Broadcast update: {:?}", update);
                    // send the update to all of the clients
                    client_registry_capture1.broadcast(GridClientResponse::GridUpdated(update));
                    future::ok(())
                });

            let client_registry_capture2 = client_registry.clone();
            let server = listener.incoming().for_each(|(socket, peer_addr)| {
                // capture/clone the grid_message_sender from the grid itself
                let server_grid_message_sender = grid_inbox.clone();

                let mut rng = rand::thread_rng();
//                let initial_pos = rng.gen();
//                let connections = connections.clone();

                let server_stuff = server_stuff.clone();
                let handle_client = handshake.call(socket, peer_addr, codecs::GridTextCodec).and_then(move |(handshake_out, io)| {
                    server_stuff.handle_client(handshake_out, io)
                });

//                let handshake = handshake.call(socket, peer_addr, codecs::GridTextCodec).and_then(move |(handshake_out, io)| {
//                    let ClientHandshakeOut{ uid, name, addr } = handshake_out;
//
//                    let (client, client_broadcast) = Client::new(uid, name, addr);
//
//                    future::ok((client, io, client_broadcast))
//                });

                // clone the connections Arc so we can move it into the closure
//                let client_registry_capture3 = client_registry_capture2.clone();
//                // clone the grid_message_sender Rc so we can move it into the closure
//                let client_grid_message_sender = server_grid_message_sender.clone();
//                let handle_client = handshake.and_then(move |(client, io, client_broadcast)| {
//                    let Client{ uid, name, .. } = client;
//                    let socket_out = io.writer;
//                    let socket_in = io.reader;
//                    let client_inbox = client.inbox.clone();
//                    // Register the client in the "connections" map.
//                    client_registry_capture3.register(&client);
//
//                    // Notify the Grid of the new player
//                    client_grid_message_sender.send(GridMessage::Connect(uid, name, initial_pos)).unwrap();
//
//                    // clone the client_grid_message_sender to capture in the responses_or_hangups closure
//                    let grid_sender_for_incoming = client_grid_message_sender.clone();
//
//                    // A Future that handles every message coming from the client's socket.
//                    // The Future type should have `()` for both Item and Error.
//                    let handle_incoming = socket_in
//                        .for_each(move |msg| {
//                            println!("Received message from {} - {:?}", uid, msg);
//                            let action: Result<GridMessage, GridServerHangup> = match msg {
//                                GridClientRequest::LoginAs(_) => Err(GridServerHangup::UnexpectedLogin),
//                                GridClientRequest::Unrecognized(_) => Err(GridServerHangup::UnrecognizedRequest),
//                                GridClientRequest::MoveRel(d_pos) => Ok(GridMessage::MoveRel(uid, d_pos)),
//                            };
//
//                            match action {
//                                Ok(grid_msg) => {
//                                    let send_result = grid_sender_for_incoming
//                                        .send(grid_msg)
//                                        .map_err(|_| {
//                                            client_inbox.push(GridClientResponse::Hangup(GridServerHangup::InternalError));
//                                            fake_io_error("Hangup after failure to send grid message")
//                                        });
//                                    future::result(send_result)
//                                },
//                                Err(hangup) => {
//                                    println!("About to hang up on client {} because: {:?}", uid, hangup);
//                                    client_inbox.push(GridClientResponse::Hangup(hangup));
//                                    future::err(fake_io_error("Hangup on misbehaving client"))
//                                }
//                            }
//                        })
//                        .map_err(|_| ());
//
//                    // A stream representing the messages sent to the client from external sources.
//                    // The `send_all` method (used later) wants the Stream to have Error=io::Error, so we map it from ()
//                    let outgoing_messages = client_broadcast
//                        .map_err(|_| fake_io_error("Error receiving broadcast message"));
//
//                    // A Future that handles every message being sent to the client's socket.
//                    // Future Future type should have `()` as both Item and Error.
//                    let handle_outgoing = socket_out
//                        .send_all(outgoing_messages)
//                        .map(|_| ())
//                        .map_err(|_| ());
//
//                    // A future combining `handle_incoming` and `handle_outgoing`.
//                    // This will complete when either of the two parts complete, i.e. when the client disconnects.
//                    let handle_io = handle_incoming.select(handle_outgoing).map(|(_first_result, _other_future)| ());
//
//                    // capture client_grid_message_sender again for the handle_io.then closure
//                    let disconnect_message_sender = client_grid_message_sender.clone();
//
//                    // Make sure to de-register the client once the IO is finished (i.e. disconnected)
//                    // This Future will be the return for this closure, representing the entire client lifecycle.
//                    let client_registry_capture4 = client_registry_capture3.clone();
//                    handle_io.then(move |_| {
//                        let removed = client_registry_capture4.remove(&uid);
//                        if removed.is_some() {
//                            println!("successfully de-registered the disconnected client");
//                        } else {
//                            println!("failed to de-register the disconnected client :(");
//                        }
//                        disconnect_message_sender.send(GridMessage::Disconnect(uid)).unwrap();
//                        Ok(())
//                    })
//                });
//
                handle.spawn(handle_client);
                Ok(())
            });

            let server_and_broadcast = consume_grid_broadcast.map_err(|_| fake_io_error("uh oh")).select(server);

            let x = core.run(server_and_broadcast);
            match x {
                Ok(_) => (),
                Err(_e) => println!("Server crashed!"),
            }
        }
        _ => {
            println!("usage: {} server", std::env::args().next().unwrap());
        }
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
        for (uid, inbox) in clients.iter() {
            println!("broadcasting {:?} to client {}", msg, uid);
            inbox.push(msg);
        }
    }
}

#[derive(Clone)]
struct ServerStuff {
    clients: ClientRegistry,
    grid_inbox: GridInbox,
}

impl ServerStuff {
    fn handle_client<T, C>(&self, handshake_result: ClientHandshakeOut, io: ClientIO<T, C>) -> Box<Future<Item = (), Error = ()>>
        where T: AsyncRead + AsyncWrite + 'static,
              C: Decoder<Item = GridClientRequest, Error = io::Error> + Encoder<Item = GridClientResponse, Error = io::Error> + 'static
    {
        let self_ref = self.clone();
        let raw_future = self.on_connect(handshake_result, io).and_then(move |(client_data, io)| {
            // Grab the R/W & I/O objects from the connect result.
            // Note that the method's `io` parameter is consumed by `on_connect`, since that method could potentially do some IO.
            let ClientIO{ reader, writer } = io;
            let (client_ident, client_input_data, client_output_data) = client_data;

            // Run the input and output handlers to get a Future for each respective completion/hangup.
            // Clone the `client_ident` because the handler implementations will want to move the value, and we need to be able to use it later.
            let handled_input = self_ref.handle_incoming(client_ident.clone(), client_input_data, reader);
            let handled_output = self_ref.handle_outgoing(client_ident.clone(), client_output_data, writer);

            // Select the first of the IO futures to either finish or fail.
            // We don't care about the return type, as it will be discarded by the disconnect handler.
            // We do need to make sure the error type stays as (), so we need to map it from the Select combinator's special error type.
            let handled_io = handled_input.select(handled_output)
                .map_err(|(_first_err, _next_future)| ());

            // Once the IO is done, run the disconnect handler.
            // IMPORTANT: use `then`, not `and_then`, because we want this closure to run regardless of the success or error of the IO handler.
            // If the client goofs up and we hang up on them, that's an error, but we still want to de-register that client!
            handled_io.then(move |_| {
                self_ref.handle_disconnect(client_ident)
            })
        });

        Box::new(raw_future)
    }

    fn on_connect<T: 'static, C: 'static>(&self, handshake_result: ClientHandshakeOut, io: ClientIO<T, C>) -> Box<Future<Item = ((Client, (), ClientBroadcast), ClientIO<T, C>), Error = ()>> {
        let ClientHandshakeOut{ uid, name, addr } = handshake_result;
        let (client, client_broadcast) = Client::new(uid, name, addr);

        // Register the client in the "connections" map.
        self.clients.register(&client);

        // Request a new "Player" be added to the grid at point {0, 0}
        self.grid_inbox.send(GridMessage::Connect(uid, name, GridPoint(0, 0))).unwrap();

        Box::new(future::ok(((client, (), client_broadcast), io)))
    }

    fn handle_incoming<T, C>(&self, client_ident: Client, client_input_data: (), input: SplitStream<Framed<T, C>>) -> Box<Future<Item = (), Error = ()>>
        where T: AsyncRead + AsyncWrite + 'static,
              C: Decoder<Item = GridClientRequest, Error = io::Error> + Encoder<Item = GridClientResponse, Error = io::Error> + 'static
    {
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

    fn handle_outgoing<T, C>(&self, client_ident: Client, client_output_data: ClientBroadcast, output: SplitSink<Framed<T, C>>) -> Box<Future<Item = (), Error = ()>>
        where T: AsyncRead + AsyncWrite + 'static,
              C: Decoder<Item = GridClientRequest, Error = io::Error> + Encoder<Item = GridClientResponse, Error = io::Error> + 'static
    {
        let client_broadcast = client_output_data;
        let outgoing_messages = client_broadcast.map_err(|_| fake_io_error("Error receiving broadcast message"));
        let raw_future = output.send_all(outgoing_messages)
            .map(|_| ())
            .map_err(|_| ());

        Box::new(raw_future)
    }

    fn handle_disconnect(&self, client_ident: Client) -> Box<Future<Item = (), Error = ()>> {
        let uid = client_ident.uid;
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

// implementation for "simple server" tutorial, part 2 (Protocol)
pub struct LineProto;

impl <T: AsyncRead + AsyncWrite + 'static> ServerProto<T> for LineProto {
    // Decoder::Item
    type Request = String;

    // Encoder::Item
    type Response = String;

    // boilerplate / magic
    type Transport = Framed<T, codecs::LineCodec>;
    type BindTransport = Result<Self::Transport, io::Error>;

    fn bind_transport(&self, io: T) -> Self::BindTransport {
        Ok(io.framed(codecs::LineCodec))
    }
}


pub struct Echo;

impl Service for Echo {
    // request/response types must correspond to the  protocol types
    type Request = String;
    type Response = String;

    // tutorial says: For non-streaming protocols, service errors are always io::Error
    type Error = io::Error;

    type Future = Box<Future<Item = Self::Response, Error = Self::Error>>;
    // compute a future of the response. No actual IO needed, so just use future::ok
    fn call(&self, req: Self::Request) -> Self::Future {
        Box::new(future::ok(req))
    }
}

pub struct EchoReverse;

impl Service for EchoReverse {
    type Request = String;
    type Response = String;
    type Error = io::Error;
    type Future = Box<Future<Item = String, Error = io::Error>>;
    fn call(&self, req: Self::Request) -> Self::Future {
        let rev: String = req.chars().rev().collect();
        Box::new(future::ok(rev))
    }
}