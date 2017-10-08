// A tiny async echo server with tokio-core
extern crate bytes;
extern crate futures;
extern crate rand;
extern crate tokio_core;
extern crate tokio_io;
extern crate tokio_proto;
extern crate tokio_service;

mod codecs;

use bytes::BytesMut;

use futures::{future, Future, Stream, Sink};
use futures::stream;
use futures::sync::mpsc::{unbounded as stream_channel, UnboundedSender, UnboundedReceiver};

use rand::{Rand, Rng};

use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::iter;
use std::net::SocketAddr;
use std::rc::Rc;
use std::str;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Sender, Receiver};
use std::thread;

use tokio_core::net::TcpListener;
use tokio_core::reactor::Core;
use tokio_io::codec::{Encoder, Decoder};
use tokio_io::{AsyncRead, AsyncWrite};
use tokio_io::codec::Framed;
use tokio_proto::pipeline::ServerProto;
use tokio_proto::TcpServer;
use tokio_service::{Service, NewService};


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
            let uid_counter = AtomicUsize::new(0);
            let (mut grid,grid_message_sender,grid_update_receiver) = Grid::new();
            let grid_message_sender = Rc::new(grid_message_sender);
            thread::spawn(move || grid.run());

//            let connections = Rc::new(RefCell::new(HashMap::new()));
            let connections: Arc<Mutex<HashMap<usize, Client, _>>> = Arc::new(Mutex::new(HashMap::new()));

            let connections_for_broadcast = connections.clone();
            let connections_for_server = connections.clone();

            let broadcast_grid_updates =
                grid_update_receiver.for_each(move |update| {
                    println!("Broadcast update: {:?}", update);

                    // send the update to all of the clients
                    let clients = connections_for_broadcast.lock().unwrap();
                    for ref client in clients.values() {
                        client.sender.unbounded_send(update);
                    }
                    future::ok(())
                });
            handle.spawn(broadcast_grid_updates);

            let server = listener.incoming().for_each(|(socket, peer_addr)| {
                let mut rng = rand::thread_rng();
                let uid = uid_counter.fetch_add(1, Ordering::SeqCst);
                let player_id = rng.gen_ascii_chars().next().unwrap();
                let initial_pos = rng.gen();
                let (writer, reader) = socket.framed(codecs::LineCodec).split();
                let connections = connections.clone();

                // channel for outside sources to send messages to the socket
                let (client_grid_updates, grid_updates) = stream_channel::<GridUpdate>();

                let client = Client {
                    uid,
                    player_id,
                    addr: peer_addr,
                    sender: client_grid_updates,
                };
//                let player_id = player_id_gen.next().unwrap();
                println!("User connected from {} with ID '{}'", peer_addr, player_id);
                //
                {
                    let mut connections_inner = connections.lock().unwrap();
                    connections_inner.insert(uid,client);
                }
//                let (t_send, t_recv) = futures::sync::mpsc::unbounded();

                grid_message_sender.send(GridMessage::Connect(player_id, initial_pos)).unwrap();

                let handle_messages = reader.for_each(move |msg| {
                    let player_id = player_id.clone();
                    println!("Received message from '{}': {}", player_id, msg);
                    // TODO: handle the message by sending a GridMessage to the `client_grid_updates` channel
                    future::ok(())
                }).map_err(|_| ());

                let grid_update_strings = grid_updates
                    .map(|upd| format!("{:?}", upd) )
                    .map_err(|_| io::Error::new(io::ErrorKind::Other, "idk lol"));
                let handle_updates = writer.send_all(grid_update_strings)
                    .map_err(|_| ())
                    .map(|_| ());

                let grid_message_sender = grid_message_sender.clone();
                let handle_everything = handle_messages.select(handle_updates).then(move |_| {
                    let player_id = player_id.clone();
                    println!("User '{}' disconnected", player_id);

                    // remove the client from the clients registry
                    {
                        let mut connections_inner = connections.lock().unwrap();
                        let removed = connections_inner.remove(&uid);
                        if removed.is_some() {
                            println!("successfully de-registered the disconnected client");
                        } else {
                            println!("failed to de-register the disconnected client :(");
                        }
                        grid_message_sender.send(GridMessage::Disconnect(player_id));
                    }
                    Ok(())
                });

//                handle.spawn(handle_messages.map_err(|_| ()));

//                let service = Echo;
//                let responses = reader.and_then(move |req| service.call(req));
//                let stuff = writer2.send_all(responses).then(move |_| {
//                    println!("User '{}' disconnected", player_id);
//                    Ok(())
//                });
                handle.spawn(handle_everything);
                Ok(())
            });

            core.run(server);
        }
        _ => {
            println!("usage: {} server", std::env::args().next().unwrap());
        }
    }
}

struct Client {
    uid: usize,
    player_id: char,
    addr: SocketAddr,
    sender: UnboundedSender<GridUpdate>
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

type PlayerId = char;

struct Player {
    id: PlayerId,
    pos: GridPoint,
    is_newly_connected: bool,
    is_modified: bool,
}
impl Player {
    fn new(id: PlayerId, pos: GridPoint) -> Player {
        Player {
            id,
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
            out.unbounded_send(GridUpdate::Connected(self.id, self.pos));
        }
        if self.is_modified || replay_inits {
            out.unbounded_send(GridUpdate::MovedTo(self.id, self.pos));
        }
        self.is_newly_connected = false;
        self.is_modified = false;
    }
}

#[derive(Debug)]
enum GridMessage {
    Connect(PlayerId, GridPoint),
    Disconnect(PlayerId),
    MoveRel(PlayerId, GridPoint),
}

#[derive(Debug, Clone, Copy)]
enum GridUpdate {
    Connected(PlayerId, GridPoint),
    MovedTo(PlayerId, GridPoint),
    Disconnected(PlayerId),
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
                    GridMessage::Connect(id, pos) => {
                        let player = Player::new(id, pos);
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


//mod direction {
//    use ::GridPoint;
//    pub const UP: GridPoint = GridPoint(0, -1);
//    pub const DOWN: GridPoint = GridPoint(0, 1);
//    pub const LEFT: GridPoint = GridPoint(-1, 0);
//    pub const RIGHT: GridPoint = GridPoint(1, 0);
//}

//pub struct GridCodec;

//impl Decoder for GridCodec {
//    type Item = Result<GridRequest, String>;
//    type Error = io::Error;
//
//    fn decode(&mut self, buf: &mut BytesMut) -> io::Result<Self::Item> {
//        LineCodec.decode(buf).map(|line| {
//            let line_ref: Option<&str> = line.as_ref().map(String::as_ref);
//            match line_ref {
//                Some("up") => Ok(GridRequest::MoveRel(direction::UP)),
//                Some("down") => Ok(GridRequest::MoveRel(direction::DOWN)),
//                Some("left") => Ok(GridRequest::MoveRel(direction::LEFT)),
//                Some("right") => Ok(GridRequest::MoveRel(direction::RIGHT)),
//                Some(s) => Err(s),
//                None => None,
//            }
//        })
//    }
//}
//impl Encoder for GridCodec {
//    type Item = GridResponse;
//    type Error = io::Error;
//
//    fn encode(&mut self, item: Self::Item, buf: &mut BytesMut) -> Result<(), Self::Error> {
//        match item {
//            GridResponse::Ack => buf.extend(b"ok"),
//            GridResponse::NoAck => buf.extend(b"invalid"),
//        }
//        buf.extend(b"\n");
//        Ok(())
//    }
//}

//pub struct GridProto;
//
//impl <T: AsyncRead + AsyncWrite + 'static> ServerProto<T> for GridProto {
//    type Request = Result<GridRequest, String>;
//    type Response = GridResponse;
//    type Transport = Framed<T, GridCodec>;
//    type BindTransport = Result<Self::Transport, io::Error>;
//    fn bind_transport(&self, io: T) -> Self::BindTransport {
//        let transport = io.framed(GridCodec);
//
//        Box::new(transport.into_future()
//            .map_err(|(e, _)| e)
//            .and_then(|(line, transport)| {
//
//            })
//        )
////        Ok(io.framed(GridCodec))
//    }
//}

//pub struct GridService;
//impl Service for GridService {
//    type Request = Result<GridRequest, String>;
//    type Response = GridResponse;
//    type Error = io::Error;
//    type Future = Box<Future<Item = Self::Response, Error = Self::Error>>;
//
//    fn call(&self, req: Self::Request) -> Self::Future {
//        let response = match req {
//            Ok(request) => {
//
//            }
//        }
//        unimplemented!()
//    }
//}

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


// implementation for "simple server" tutorial, part 3 (Service)

/// The echo "service". Doesn't actually hold data since it's a stateless echo server.
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