use futures::{Poll, Stream};
use futures::sync::mpsc::{unbounded as stream_channel, UnboundedSender, UnboundedReceiver};

use rand::{Rand, Rng};

use std::rc::Rc;
use std::sync::mpsc::{channel, Sender, Receiver};

#[derive(Debug, Clone, Copy)]
pub struct GridPoint(pub i32, pub i32);
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

pub type PlayerName = char;
pub type PlayerUid = u64;

pub struct Player {
    id: PlayerUid,
    name: PlayerName,
    pos: GridPoint,
    is_newly_connected: bool,
    is_modified: bool,
}
impl Player {
    pub fn new(id: PlayerUid, name: PlayerName, pos: GridPoint) -> Player {
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
pub enum GridMessage {
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

/// A stream of `GridUpdate` events produced by a `Grid`.
///
/// As the game loop causes things to move around and change state,
/// the `Grid` will emit events to this stream.
///
/// Because the stream has an unlimited buffer, it must be consumed
/// (i.e. polled) to avoid using too much memory.
#[must_use]
pub struct GridBroadcast {
    inner: UnboundedReceiver<GridUpdate>
}

impl Stream for GridBroadcast {
    type Item = GridUpdate;
    type Error = ();
    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        self.inner.poll()
    }
}

/// Entry point for passing `GridMessages` to a `Grid`.
///
/// Messages passed in this manner are the only means of sending user input
/// to the `Grid`. Each message will be handled during the next available
/// iteration of the game loop.
#[derive(Clone)]
pub struct GridInbox {
    inner: Rc<Sender<GridMessage>>,
}
impl GridInbox {
    pub fn send(&self, msg: GridMessage) -> Result<(), ()> {
        self.inner.send(msg).map_err(|_| ())
    }
}

pub struct Grid {
    players: Vec<Player>,
    incoming: Receiver<GridMessage>,
    outgoing: UnboundedSender<GridUpdate>,
}

impl Grid {
    pub fn new() -> (Grid, GridInbox, GridBroadcast) {
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
    pub fn run(&mut self) {
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
