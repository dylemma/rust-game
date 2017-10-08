use bytes::BytesMut;
use std::io;
use std::str;
use tokio_io::codec::{Encoder, Decoder};

use super::{GridClientRequest, GridClientResponse, GridPoint, PlayerUid};

pub struct LineCodec;

/// Decoder that grabs all of the bytes up until the first '\n' and interprets them as a UTF-8 String.
impl Decoder for LineCodec {
    type Item = String;
    type Error = io::Error;

    fn decode(&mut self, buf: &mut BytesMut) -> io::Result<Option<String>> {
        if let Some(i) = buf.iter().position(|&b| b == b'\n') {
            // grab the first `i` bytes of the buffer, modifying the buffer so it starts after those bytes
            let line = buf.split_to(i);

            // nobody cares about the '\n', so split that away as well
            buf.split_to(1);

            match str::from_utf8(&line) {
                Ok(s) => Ok(Some(s.to_string())),
                Err(_) => Err(io::Error::new(io::ErrorKind::Other, "invalid UTF-8")),
            }
        } else {
            Ok(None)
        }
    }
}

/// Encoder for line-based messages
impl Encoder for LineCodec {
    type Item = String;
    type Error = io::Error;

    fn encode(&mut self, msg: String, buf: &mut BytesMut) -> io::Result<()> {
        buf.extend(msg.as_bytes());
        buf.extend(b"\n");
        Ok(())
    }
}

pub struct GridTextCodec;

impl Decoder for GridTextCodec {
    type Item = GridClientRequest;
    type Error = io::Error;

    fn decode(&mut self, buf: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        LineCodec.decode(buf).map(|line_opt| line_opt.map(|line| {
            let line = line.trim_right();

            if line.starts_with("login:") {
                if let Some(player_name) = line.chars().nth(6) {
                    return GridClientRequest::LoginAs(player_name);
                }
            }

            match line {
                "up" => return GridClientRequest::MoveRel(GridPoint(0, -1)),
                "down" => return GridClientRequest::MoveRel(GridPoint(0, 1)),
                "left" => return GridClientRequest::MoveRel(GridPoint(-1, 0)),
                "right" => return GridClientRequest::MoveRel(GridPoint(1, 0)),
                _ => ()
            }

            GridClientRequest::Unrecognized(line.to_string())
        }))
    }
}

impl Encoder for GridTextCodec {
    type Item = GridClientResponse;
    type Error = io::Error;

    fn encode(&mut self, item: Self::Item, buf: &mut BytesMut) -> Result<(), Self::Error> {
        LineCodec.encode(format!("{:?}", item), buf)
    }
}