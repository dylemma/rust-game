// A tiny async echo server with tokio-core
extern crate bytes;
extern crate futures;
extern crate tokio_core;
extern crate tokio_io;
extern crate tokio_proto;
extern crate tokio_service;

// imports for Step 1: Implement a Codec
use std::io;
use std::str;
use bytes::BytesMut;
use tokio_io::codec::{Encoder, Decoder};

// imports for Step 2: Specify the Protocol
use tokio_proto::pipeline::ServerProto;
use tokio_io::{AsyncRead, AsyncWrite};
use tokio_io::codec::Framed;

// imports for Step 3: Implement a Service
use tokio_service::Service;
use futures::{future, Future};

// imports for main
use tokio_proto::TcpServer;

fn main() {
    let addr = "127.0.0.1:12345".parse().unwrap();
    let server = TcpServer::new(LineProto, addr);
    server.serve(|| Ok(EchoReverse));
}

// implementation for "simple server" tutorial, part 1 (Codec)

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

// implementation for "simple server" tutorial, part 2 (Protocol)
pub struct LineProto;

impl <T: AsyncRead + AsyncWrite + 'static> ServerProto<T> for LineProto {
    // Decoder::Item
    type Request = String;

    // Encoder::Item
    type Response = String;

    // boilerplate / magic
    type Transport = Framed<T, LineCodec>;
    type BindTransport = Result<Self::Transport, io::Error>;

    fn bind_transport(&self, io: T) -> Self::BindTransport {
        Ok(io.framed(LineCodec))
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