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
            let (mut grid,direct_grid_message_sender,grid_update_receiver) = Grid::new();
            let outer_grid_message_sender = Rc::new(direct_grid_message_sender);
            thread::spawn(move || grid.run());

            let mut handshake = ClientHandshake::new();

            let connections: Arc<Mutex<HashMap<u64, Client, _>>> = Arc::new(Mutex::new(HashMap::new()));

            let connections_for_broadcast = connections.clone();
            let broadcast_grid_updates =
                grid_update_receiver.for_each(move |update| {
                    println!("Broadcast update: {:?}", update);

                    // send the update to all of the clients
                    let clients = connections_for_broadcast.lock().unwrap();
                    for client in clients.values() {
                        client.sender.unbounded_send(GridClientResponse::GridUpdated(update)).unwrap();
                    }
                    future::ok(())
                });

            let server = listener.incoming().for_each(|(socket, peer_addr)| {
                // capture/clone the grid_message_sender from the grid itself
                let server_grid_message_sender = outer_grid_message_sender.clone();

                let mut rng = rand::thread_rng();
                let player_name = rng.gen_ascii_chars().next().unwrap();
                let initial_pos = rng.gen();
                let connections = connections.clone();
                // channel for outside sources to send messages to the socket
                let (client_grid_updates, grid_updates) = stream_channel::<GridClientResponse>();

                let handshake = handshake.call(socket, peer_addr, codecs::GridTextCodec).and_then(move |handshake_out| {
                    let ClientHandshakeOut{ uid, name, io } = handshake_out;
                    let (writer, reader) = io.split();
                    let client = Client {
                        uid,
                        player_name,
                        addr: peer_addr,
                        sender: Rc::new(client_grid_updates),
                    };
                    future::ok((client, writer, reader))
                });

                // clone the connections Arc so we can move it into the closure
                let connections_1 = connections.clone();
                // clone the grid_message_sender Rc so we can move it into the closure
                let client_grid_message_sender = server_grid_message_sender.clone();
                let handle_client = handshake.and_then(move |(client, socket_out, socket_in)| {
                    let Client{ uid, player_name, .. } = client;
                    let responder = client.sender.clone();
                    // Register the client in the "connections" map.
                    //  - Get a lock on the connections map to insert the newly-connected client.
                    //  - Use a block scope to ensure the lock drops quickly.
                    {
                        let mut connections_inner = connections_1.lock().unwrap();
                        connections_inner.insert(uid,client);
                    }

                    // Notify the Grid of the new player
                    client_grid_message_sender.send(GridMessage::Connect(uid, player_name, initial_pos)).unwrap();


                    // clone the client_grid_message_sender to capture in the responses_or_hangups closure
                    let grid_sender_for_incoming = client_grid_message_sender.clone();

                    // A Future that handles every message coming from the client's socket.
                    // The Future type should have `()` for both Item and Error.
                    let handle_incoming = socket_in
                        .for_each(move |msg| {
                            println!("Received message from {} - {:?}", uid, msg);
                            let action: Result<GridMessage, GridServerHangup> = match msg {
                                GridClientRequest::LoginAs(_) => Err(GridServerHangup::UnexpectedLogin),
                                GridClientRequest::Unrecognized(_) => Err(GridServerHangup::UnrecognizedRequest),
                                GridClientRequest::MoveRel(d_pos) => Ok(GridMessage::MoveRel(uid, d_pos)),
                            };

                            match action {
                                Ok(grid_msg) => {
                                    let send_result = grid_sender_for_incoming
                                        .send(grid_msg)
                                        .map_err(|_| {
                                            responder.unbounded_send(GridClientResponse::Hangup(GridServerHangup::InternalError)).unwrap();
                                            fake_io_error("Hangup after failure to send grid message")
                                        });
                                    future::result(send_result)
                                },
                                Err(hangup) => {
                                    println!("About to hang up on client {} because: {:?}", uid, hangup);
                                    responder.unbounded_send(GridClientResponse::Hangup(hangup)).unwrap();
                                    future::err(fake_io_error("Hangup on misbehaving client"))
                                }
                            }
                        })
                        .map_err(|err| ());

                    // A stream representing the messages sent to the client from external sources.
                    // The `send_all` method (used later) wants the Stream to have Error=io::Error, so we map it from ()
                    let outgoing_messages = grid_updates
                        .map_err(|_| fake_io_error("Error receiving broadcast message"));

                    // A Future that handles every message being sent to the client's socket.
                    // Future Future type should have `()` as both Item and Error.
                    let handle_outgoing = socket_out
                        .send_all(outgoing_messages)
                        .map(|_| ())
                        .map_err(|_| ());

                    // A future combining `handle_incoming` and `handle_outgoing`.
                    // This will complete when either of the two parts complete, i.e. when the client disconnects.
                    let handle_io = handle_incoming.select(handle_outgoing).map(|(_first_result, _other_future)| ());

                    // capture client_grid_message_sender again for the handle_io.then closure
                    let disconnect_message_sender = client_grid_message_sender.clone();

                    // Make sure to de-register the client once the IO is finished (i.e. disconnected)
                    // This Future will be the return for this closure, representing the entire client lifecycle.
                    handle_io.then(move |_| {
                        {
                            let mut connections_inner = connections.lock().unwrap();
                            let removed = connections_inner.remove(&uid);
                            if removed.is_some() {
                                println!("successfully de-registered the disconnected client");
                            } else {
                                println!("failed to de-register the disconnected client :(");
                            }
                            disconnect_message_sender.send(GridMessage::Disconnect(uid)).unwrap();
                        }
                        Ok(())
                    })
                });

                handle.spawn(handle_client);
                Ok(())
            });

            let server_and_broadcast = broadcast_grid_updates.map_err(|_| fake_io_error("uh oh")).select(server);

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

struct Client {
    uid: PlayerUid,
    player_name: PlayerName,
    addr: SocketAddr,
    sender: Rc<UnboundedSender<GridClientResponse>>
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
    outgoing: UnboundedSender<GridUpdate>
}

impl Grid {
    fn new() -> (Grid, Sender<GridMessage>, UnboundedReceiver<GridUpdate>) {
        let (grid_message_sender, grid_message_receiver) = channel();
        let (grid_update_sender, grid_update_receiver) = stream_channel();
        let grid = Grid {
            players: Vec::new(),
            incoming: grid_message_receiver,
            outgoing: grid_update_sender
        };
        (grid, grid_message_sender, grid_update_receiver)
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

struct ClientHandshakeOut<T, C> {
    uid: PlayerUid,
    name: PlayerName,
    io: Framed<T, C>
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

    fn call<T, C>(&mut self, socket: T, peer_addr: SocketAddr, codec: C) -> Box<Future<Item = ClientHandshakeOut<T, C>, Error = ()>>
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
                        println!("Unsuccessful login attempt from {}: {:?}", peer_addr, other);
                        Err(fake_io_error("Expected login attempt"))
                    }
                };

                // Assuming the login was Ok, generate a PlayerUid for the client...
                future::result(login_attempt).and_then(move |name| {
                    let uid: u64 = uid_counter.fetch_add(1, Ordering::SeqCst) as u64;
                    println!("Successful login handshake by client {} with name {}", uid, name);

                    // Send the client an acknowledgement containing the PlayerUid
                    writer_cont.send(GridClientResponse::LoggedIn(uid)).and_then(move |writer_cont| {
                        future::ok(ClientHandshakeOut{
                            uid,
                            name,
                            io: writer_cont.reunite(read_cont).unwrap()
                        })
                    })
                })
            })
        }).map_err(|_| ());

        Box::new(raw_future)
    }
}


// implementation for "simple server" tutorial, part 1 (Codec)

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