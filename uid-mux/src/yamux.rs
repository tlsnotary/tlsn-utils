//! Yamux multiplexer.
//!
//! This module provides a [`yamux`](https://crates.io/crates/yamux) wrapper which implements [`UidMux`](crate::UidMux).

use std::{
    collections::HashMap,
    fmt,
    future::IntoFuture,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll},
};

use async_trait::async_trait;
use futures::{
    ready, stream::FuturesUnordered, AsyncRead, AsyncWrite, Future, FutureExt, StreamExt,
};
use tokio::sync::{oneshot, Notify};
use yamux::Connection;

use crate::{
    future::{ReadId, ReturnStream},
    log::{debug, error, info, trace},
    InternalId, UidMux,
};

pub use yamux::{Config, ConnectionError, Mode, Stream};

type Result<T, E = ConnectionError> = std::result::Result<T, E>;

#[derive(Debug, Clone, Copy)]
enum Role {
    Client,
    Server,
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Role::Client => write!(f, "Client"),
            Role::Server => write!(f, "Server"),
        }
    }
}

/// A yamux multiplexer.
#[derive(Debug)]
pub struct Yamux<Io> {
    role: Role,
    conn: Connection<Io>,
    queue: Arc<Mutex<Queue>>,
    close_notify: Arc<Notify>,
    shutdown_notify: Arc<AtomicBool>,
}

#[derive(Debug, Default)]
struct Queue {
    waiting: HashMap<InternalId, oneshot::Sender<Stream>>,
    ready: HashMap<InternalId, Stream>,
}

impl<Io> Yamux<Io> {
    /// Returns a new control handle.
    pub fn control(&self) -> YamuxCtrl {
        YamuxCtrl {
            queue: self.queue.clone(),
            close_notify: self.close_notify.clone(),
            shutdown_notify: self.shutdown_notify.clone(),
        }
    }
}

impl<Io> Yamux<Io>
where
    Io: AsyncWrite + AsyncRead + Unpin,
{
    /// Create a new yamux multiplexer.
    pub fn new(io: Io, config: Config, mode: Mode) -> Self {
        let role = match mode {
            Mode::Client => Role::Client,
            Mode::Server => Role::Server,
        };

        Self {
            role,
            conn: Connection::new(io, config, mode),
            queue: Default::default(),
            close_notify: Default::default(),
            shutdown_notify: Default::default(),
        }
    }
}

impl<Io> IntoFuture for Yamux<Io>
where
    Io: AsyncWrite + AsyncRead + Unpin,
{
    type Output = Result<()>;
    type IntoFuture = YamuxFuture<Io>;

    fn into_future(self) -> Self::IntoFuture {
        YamuxFuture {
            role: self.role,
            conn: self.conn,
            incoming: Default::default(),
            outgoing: Default::default(),
            queue: self.queue,
            closed: false,
            close_notify: self.close_notify,
            shutdown_notify: self.shutdown_notify,
        }
    }
}

/// A yamux connection future.
#[derive(Debug)]
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct YamuxFuture<Io> {
    role: Role,
    conn: Connection<Io>,
    /// Pending incoming streams, waiting for ids to be received.
    incoming: FuturesUnordered<ReadId<Stream>>,
    /// Pending outgoing streams, waiting to send ids and return streams
    /// to callers.
    outgoing: FuturesUnordered<ReturnStream<Stream>>,
    queue: Arc<Mutex<Queue>>,
    closed: bool,
    close_notify: Arc<Notify>,
    shutdown_notify: Arc<AtomicBool>,
}

impl<Io> YamuxFuture<Io>
where
    Io: AsyncWrite + AsyncRead + Unpin,
{
    fn poll_client(&mut self, cx: &mut Context<'_>) -> Poll<Result<()>> {
        if let Poll::Ready(stream) = self.conn.poll_next_inbound(cx).map(Option::transpose)? {
            self.closed = true;
            info!("({}): mux connection closed", self.role);
            if stream.is_some() {
                error!("({}): client mux received incoming stream", self.role);
                return Poll::Ready(Err(std::io::Error::other(
                    "client mode can not accept incoming streams",
                )
                .into()));
            }
        }

        // Open new outgoing streams.
        let mut queue = self.queue.lock().unwrap();
        while !self.closed && !queue.waiting.is_empty() {
            if let Poll::Ready(stream) = self.conn.poll_new_outbound(cx)? {
                let id = *queue.waiting.keys().next().unwrap();
                let sender = queue.waiting.remove(&id).unwrap();

                debug!("({}): opened new stream: {}", self.role, id);

                self.outgoing.push(ReturnStream::new(id, stream, sender));
            }
        }

        // Poll all outgoing streams.
        while let Poll::Ready(Some(())) =
            self.outgoing.poll_next_unpin(cx).map(Option::transpose)?
        {
            trace!("({}): finished processing stream", self.role)
        }

        if !self.closed && self.shutdown_notify.load(Ordering::Relaxed) {
            if let Poll::Ready(()) = self.conn.poll_close(cx)? {
                self.closed = true;
                info!("({}): mux connection closed", self.role);
            }
        }

        if self.closed {
            Poll::Ready(Ok(()))
        } else {
            Poll::Pending
        }
    }

    fn poll_server(&mut self, cx: &mut Context<'_>) -> Poll<Result<()>> {
        if let Poll::Ready(stream) = self.conn.poll_next_inbound(cx).map(Option::transpose)? {
            if let Some(stream) = stream {
                debug!("({}): received incoming stream", self.role);

                // The size of this is bounded by yamux max streams config.
                self.incoming.push(ReadId::new(stream));
            } else {
                info!("({}): mux connection closed", self.role);
                self.closed = true;
            }
        }

        while let Poll::Ready(Some((id, stream))) =
            self.incoming.poll_next_unpin(cx).map(Option::transpose)?
        {
            debug!("({}): received stream: {}", self.role, id);
            let mut queue = self.queue.lock().unwrap();
            if let Some(sender) = queue.waiting.remove(&id) {
                trace!("({}): returning stream to caller: {}", self.role, id);
                _ = sender.send(stream);
            } else {
                trace!("({}): queueing stream: {}", self.role, id);
                queue.ready.insert(id, stream);
            }
        }

        if !self.closed && self.shutdown_notify.load(Ordering::Relaxed) {
            if let Poll::Ready(()) = self.conn.poll_close(cx)? {
                self.closed = true;
                info!("({}): mux connection closed", self.role);
            }
        }

        if self.closed && self.incoming.is_empty() {
            Poll::Ready(Ok(()))
        } else {
            Poll::Pending
        }
    }
}

impl<Io> Future for YamuxFuture<Io>
where
    Io: AsyncWrite + AsyncRead + Unpin,
{
    type Output = Result<()>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let output = match self.role {
            Role::Client => ready!(self.poll_client(cx)),
            Role::Server => ready!(self.poll_server(cx)),
        };
        self.close_notify.notify_waiters();
        Poll::Ready(output)
    }
}

/// A yamux control handle.
#[derive(Debug, Clone)]
pub struct YamuxCtrl {
    queue: Arc<Mutex<Queue>>,
    close_notify: Arc<Notify>,
    shutdown_notify: Arc<AtomicBool>,
}

impl YamuxCtrl {
    /// Close the yamux connection.
    pub fn close(&self) {
        self.shutdown_notify.store(true, Ordering::Relaxed);
    }
}

#[async_trait]
impl<Id> UidMux<Id> for YamuxCtrl
where
    Id: fmt::Debug + AsRef<[u8]> + Sync,
{
    type Stream = Stream;
    type Error = std::io::Error;

    async fn open(&self, id: &Id) -> Result<Self::Stream, Self::Error> {
        let internal_id = InternalId::new(id.as_ref());

        debug!("opening stream: {} -> {}", hex::encode(id), internal_id);

        let receiver = {
            let mut queue = self.queue.lock().unwrap();
            if let Some(stream) = queue.ready.remove(&internal_id) {
                trace!("stream already opened: {}", hex::encode(id));
                return Ok(stream);
            }

            let (sender, receiver) = oneshot::channel();
            queue.waiting.insert(internal_id, sender);

            trace!("waiting for stream: {}", hex::encode(id));

            receiver
        };

        futures::select! {
            stream = receiver.fuse() =>
                stream
                    .inspect(|_| debug!("caller received stream: {}", hex::encode(id)))
                    .inspect_err(|_| error!("connection cancelled stream: {}", hex::encode(id)))
                    .map_err(|_| {
                    std::io::Error::other(format!("connection cancelled stream: {}", hex::encode(id)))
                }),
            _ = self.close_notify.notified().fuse() => {
                error!("connection closed before stream opened: {}", hex::encode(id));
                Err(std::io::ErrorKind::ConnectionAborted.into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{AsyncReadExt, AsyncWriteExt};
    use tokio::io::duplex;
    use tokio_util::compat::TokioAsyncReadCompatExt;

    #[tokio::test]
    async fn test_yamux() {
        tracing_subscriber::fmt::init();
        let (client_io, server_io) = duplex(1024);
        let client = Yamux::new(client_io.compat(), Config::default(), Mode::Client);
        let server = Yamux::new(server_io.compat(), Config::default(), Mode::Server);

        let client_ctrl = client.control();
        let server_ctrl = server.control();

        let conn_task = tokio::spawn(async {
            futures::try_join!(client.into_future(), server.into_future()).unwrap();
        });

        futures::join!(
            async {
                let mut stream = client_ctrl.open(b"test").await.unwrap();
                stream.write_all(b"ping").await.unwrap();

                client_ctrl.close();
            },
            async {
                let mut stream = server_ctrl.open(b"test").await.unwrap();
                let mut buf = [0; 4];
                stream.read_exact(&mut buf).await.unwrap();

                server_ctrl.close();

                assert_eq!(&buf, b"ping");
            }
        );

        conn_task.await.unwrap();
    }
}
