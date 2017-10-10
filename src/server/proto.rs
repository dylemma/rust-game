use futures::Future;
use futures::stream::{SplitSink, SplitStream};

use tokio_io::codec::Framed;


pub struct ClientIO<T, C> {
    pub writer: SplitSink<Framed<T, C>>,
    pub reader: SplitStream<Framed<T, C>>,
}

/// Generalization of a service that handles clients in four separate steps:
///
///  - **Connect** takes some "handshake" data and an IO object, performing any necessary side-effects.
///  - **Handle Input** treats the client's input as a Stream, yielding a Future that completes when all input has been handled.
///  - **Handle Output** treats the client's output as a Sink, returning a Future that completes when all output has been sent.
///  - **Disconnect** performs some side-effects when either the Input or Output handler finishes or fails
///
pub trait IOService<T, C> where Self : Clone + 'static {
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