use futures;
use futures::stream::StreamFuture;
use futures::{Async, Future, Poll, StartSend, Stream, Sink};

use serde::{Deserialize, Serialize};

use std::io;

use tokio_io::{AsyncRead, AsyncWrite};
use tokio_io::io::{ReadHalf, WriteHalf};
use tokio_io::codec::length_delimited;
use bincode::{Error as BincodeError, ErrorKind as BincodeErrorKind};
use tokio_serde_bincode::{ReadBincode, WriteBincode};

/// Type hint for constructing `BincodeIO` instances.
///
/// A struct implementing this trait can be passed to `BincodeIO::new`
/// to let the compiler infer the Input and Output types.
pub trait IOMessages {
    type Input;
    type Output;
}

/// Read half of a `BincodeIO`.
pub struct BincodeStream<T: AsyncRead, I> {
    inner: ReadBincode<length_delimited::FramedRead<ReadHalf<T>>, I>
}

/// Write half of a `BincodeIO`.
pub struct BincodeSink<T: AsyncWrite, O> {
    inner: WriteBincode<length_delimited::FramedWrite<WriteHalf<T>>, O>
}

/// A Stream + Sink for messages encoded with Bincode.
///
/// The individual Stream/Sink components can be captured via the `split` method.
/// Two convenience methods (`read_one` and `write_one`) are provided to simply
/// the implementation detail of client-server handshakes.
pub struct BincodeIO<T: AsyncRead + AsyncWrite, I, O> {
    reader: BincodeStream<T, I>,
    writer: BincodeSink<T, O>
}
impl <T, I, O> BincodeIO<T, I, O>
    where T: AsyncRead + AsyncWrite,
          for<'de> I: Deserialize<'de>,
          O: Serialize
{
    /// Wrap an existing IO transport as a `BincodeIO`, using the given `protocol`
    /// to infer the Input/Output message types.
    pub fn new<P>(transport: T, _protocol: P) -> BincodeIO<T, I, O>
        where P: IOMessages<Input = I, Output = O>,
    {
        let (read_half, write_half) = transport.split();
        let read_inner = ReadBincode::<_, P::Input>::new(length_delimited::FramedRead::new(read_half));
        let write_inner = WriteBincode::<_, P::Output>::new(length_delimited::FramedWrite::new(write_half));
        let reader = BincodeStream{ inner: read_inner };
        let writer = BincodeSink{ inner: write_inner };
        BincodeIO{ reader, writer }
    }

    /// Consume the BincodeIO to get a Reader + Writer tuple.
    pub fn split(self) -> (BincodeSink<T, O>, BincodeStream<T, I>) {
        (self.writer, self.reader)
    }

    /// Read the next message from the `reader` stream.
    ///
    /// Returns a future that resolves as a tuple containing the message and the continuation of this IO object.
    /// Note that this method consumes the BincodeIO instance, so the Future must be polled in order to continue
    /// sending messages.
    pub fn read_one(self) -> BincodeIOReadOne<T, I, O> {
        BincodeIOReadOne {
            reader_future: self.reader.into_future(),
            writer: Some(self.writer),
        }
    }

    /// Write a message to the `writer` sink.
    ///
    /// Returns a future that resolves as the continuation of this IO object.
    /// Note that this method consumes the BincodeIO instance, so the Future must be polled in
    /// order to continue sending/receiving messages.
    pub fn write_one(self, msg: O) -> BincodeIOWriteOne<T, I, O> {
        BincodeIOWriteOne {
            send_future: self.writer.send(msg),
            reader: Some(self.reader),
        }
    }
}

#[must_use]
pub struct BincodeIOReadOne<T: AsyncRead + AsyncWrite, I, O> {
    reader_future: StreamFuture<BincodeStream<T, I>>,
    writer: Option<BincodeSink<T, O>>
}
impl <T: AsyncRead + AsyncWrite, I, O> Future for BincodeIOReadOne<T, I, O>
    where for<'de> I: Deserialize<'de>
{
    type Item = (I, BincodeIO<T, I, O>);
    type Error = io::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        self.reader_future.poll()
            .map_err(|(e, _reader)| e)
            .and_then(|state| {
                match state {
                    Async::NotReady => Ok(Async::NotReady),
                    Async::Ready((None, _reader)) => Err(io::ErrorKind::NotFound.into()),
                    Async::Ready((Some(item), reader)) => {
                        let writer = self.writer.take().unwrap();
                        Ok(Async::Ready((item, BincodeIO { reader, writer })))
                    }
                }
            })
    }
}

#[must_use]
pub struct BincodeIOWriteOne<T: AsyncRead + AsyncWrite, I, O: Serialize> {
    send_future: futures::sink::Send<BincodeSink<T, O>>,
    reader: Option<BincodeStream<T, I>>
}
impl <T: AsyncRead + AsyncWrite, I, O: Serialize> Future for BincodeIOWriteOne<T, I, O> {
    type Item = BincodeIO<T, I, O>;
    type Error = io::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        self.send_future.poll()
            .and_then(|state| {
                match state {
                    Async::NotReady => Ok(Async::NotReady),
                    Async::Ready(writer) => {
                        let reader = self.reader.take().unwrap();
                        Ok(Async::Ready(BincodeIO{ reader, writer }))
                    }
                }
            })
    }
}

// Stream implementation for BincodeStream
impl <T: AsyncRead, I> Stream for BincodeStream<T, I>
    where for<'de> I: Deserialize<'de>
{
    type Item = I;
    type Error = io::Error;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        self.inner.poll().map_err(downgrade_bincode_error)
    }
}

// Sink implementation for BincodeSink
impl <T: AsyncWrite, O: Serialize> Sink for BincodeSink<T, O> {
    type SinkItem = O;
    type SinkError = io::Error;

    fn start_send(&mut self, item: Self::SinkItem) -> StartSend<Self::SinkItem, Self::SinkError> {
        self.inner.start_send(item)
    }

    fn poll_complete(&mut self) -> Poll<(), Self::SinkError> {
        self.inner.poll_complete()
    }

    fn close(&mut self) -> Poll<(), Self::SinkError> {
        self.inner.close()
    }
}

fn io_error(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::Other, msg)
}

fn downgrade_bincode_error(e: BincodeError) -> io::Error {
    match *e {
        BincodeErrorKind::IoError(e) => e,
        e => io_error(&format!("Error from bincode: {:?}", e)),
    }
}