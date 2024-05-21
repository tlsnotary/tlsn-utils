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
    task::{Context, Poll, Waker},
};

use async_trait::async_trait;
use futures::{stream::FuturesUnordered, AsyncRead, AsyncWrite, Future, FutureExt, StreamExt};
use tokio::sync::{oneshot, Notify};
use yamux::Connection;

use crate::{
    future::{ReadId, ReturnStream},
    log::{debug, error, info, trace, warn},
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

#[derive(Debug)]
struct Queue {
    waiting: HashMap<InternalId, oneshot::Sender<Stream>>,
    ready: HashMap<InternalId, Stream>,
    waker: Option<Waker>,
}

impl Default for Queue {
    fn default() -> Self {
        Self {
            waiting: Default::default(),
            ready: Default::default(),
            waker: None,
        }
    }
}

impl<Io> Yamux<Io> {
    /// Returns a new control handle.
    pub fn control(&self) -> YamuxCtrl {
        YamuxCtrl {
            role: self.role,
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
    /// Creates a new yamux multiplexer.
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
            remote_closed: false,
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
    /// Whether this side has closed the connection.
    closed: bool,
    /// Whether the remote has closed the connection.
    remote_closed: bool,
    close_notify: Arc<Notify>,
    shutdown_notify: Arc<AtomicBool>,
}

impl<Io> YamuxFuture<Io>
where
    Io: AsyncWrite + AsyncRead + Unpin,
{
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, err))]
    fn client_handle_inbound(&mut self, cx: &mut Context<'_>) -> Result<()> {
        if let Poll::Ready(stream) = self.conn.poll_next_inbound(cx).map(Option::transpose)? {
            if stream.is_some() {
                error!("client mux received incoming stream");
                return Err(
                    std::io::Error::other("client mode cannot accept incoming streams").into(),
                );
            }

            info!("remote closed connection");
            self.remote_closed = true;
        }

        Ok(())
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, err))]
    fn client_handle_outbound(&mut self, cx: &mut Context<'_>) -> Result<()> {
        // Putting this in a block so the lock is released as soon as possible.
        {
            let mut queue = self.queue.lock().unwrap();
            while !queue.waiting.is_empty() {
                if let Poll::Ready(stream) = self.conn.poll_new_outbound(cx)? {
                    let id = *queue.waiting.keys().next().unwrap();
                    let sender = queue.waiting.remove(&id).unwrap();

                    debug!("opened new stream: {}", id);

                    self.outgoing.push(ReturnStream::new(id, stream, sender));
                } else {
                    break;
                }
            }

            // Set the waker so `YamuxCtrl` can wake up the connection.
            queue.waker = Some(cx.waker().clone());
        }

        while let Poll::Ready(Some(result)) = self.outgoing.poll_next_unpin(cx) {
            if let Err(err) = result {
                warn!("connection closed while opening stream: {}", err);
                self.remote_closed = true;
            } else {
                trace!("finished opening stream");
            }
        }

        Ok(())
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, err))]
    fn server_handle_inbound(&mut self, cx: &mut Context<'_>) -> Result<()> {
        while let Poll::Ready(stream) = self.conn.poll_next_inbound(cx).map(Option::transpose)? {
            let Some(stream) = stream else {
                if !self.remote_closed {
                    info!("remote closed connection");
                    self.remote_closed = true;
                }

                break;
            };

            debug!("received incoming stream");
            // The size of this is bounded by yamux max streams config.
            self.incoming.push(ReadId::new(stream));
        }

        Ok(())
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, err))]
    fn server_process_inbound(&mut self, cx: &mut Context<'_>) -> Result<()> {
        let mut queue = self.queue.lock().unwrap();
        while let Poll::Ready(Some(result)) = self.incoming.poll_next_unpin(cx) {
            match result {
                Ok((id, stream)) => {
                    debug!("received stream: {}", id);
                    if let Some(sender) = queue.waiting.remove(&id) {
                        _ = sender
                            .send(stream)
                            .inspect_err(|_| error!("caller dropped receiver"));
                        trace!("returned stream to caller: {}", id);
                    } else {
                        trace!("queuing stream: {}", id);
                        queue.ready.insert(id, stream);
                    }
                }
                Err(err) => {
                    warn!("connection closed while receiving stream: {}", err);
                    self.remote_closed = true;
                }
            }
        }

        // Set the waker so `YamuxCtrl` can wake up the connection.
        queue.waker = Some(cx.waker().clone());

        Ok(())
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, err))]
    fn handle_shutdown(&mut self, cx: &mut Context<'_>) -> Result<()> {
        // Attempt to close the connection if the shutdown notify has been set.
        if !self.closed && self.shutdown_notify.load(Ordering::Relaxed) {
            if let Poll::Ready(()) = self.conn.poll_close(cx)? {
                self.closed = true;
                info!("mux connection closed");
            }
        }

        Ok(())
    }

    fn is_complete(&self) -> bool {
        self.remote_closed || self.closed
    }

    fn poll_client(&mut self, cx: &mut Context<'_>) -> Result<()> {
        self.client_handle_inbound(cx)?;

        if !self.remote_closed {
            self.client_handle_outbound(cx)?;

            // We need to poll the inbound again to make sure the connection
            // flushes the write buffer.
            self.client_handle_inbound(cx)?;
        }

        self.handle_shutdown(cx)?;

        Ok(())
    }

    fn poll_server(&mut self, cx: &mut Context<'_>) -> Result<()> {
        self.server_handle_inbound(cx)?;
        self.server_process_inbound(cx)?;
        self.handle_shutdown(cx)?;

        Ok(())
    }
}

impl<Io> Future for YamuxFuture<Io>
where
    Io: AsyncWrite + AsyncRead + Unpin,
{
    type Output = Result<()>;

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            fields(role = %self.role),
            skip_all
        )
    )]
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.role {
            Role::Client => self.poll_client(cx)?,
            Role::Server => self.poll_server(cx)?,
        };

        if self.is_complete() {
            self.close_notify.notify_waiters();
            info!("connection complete");
            Poll::Ready(Ok(()))
        } else {
            Poll::Pending
        }
    }
}

/// A yamux control handle.
#[derive(Debug, Clone)]
pub struct YamuxCtrl {
    role: Role,
    queue: Arc<Mutex<Queue>>,
    close_notify: Arc<Notify>,
    shutdown_notify: Arc<AtomicBool>,
}

impl YamuxCtrl {
    /// Closes the yamux connection.
    pub fn close(&self) {
        self.shutdown_notify.store(true, Ordering::Relaxed);

        // Wake up the connection.
        self.queue
            .lock()
            .unwrap()
            .waker
            .as_ref()
            .map(|waker| waker.wake_by_ref());
    }
}

#[async_trait]
impl<Id> UidMux<Id> for YamuxCtrl
where
    Id: fmt::Debug + AsRef<[u8]> + Sync,
{
    type Stream = Stream;
    type Error = std::io::Error;

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            fields(role = %self.role, id = hex::encode(id)),
            skip_all,
            err
        )
    )]
    async fn open(&self, id: &Id) -> Result<Self::Stream, Self::Error> {
        let internal_id = InternalId::new(id.as_ref());

        debug!("opening stream: {}", internal_id);

        let receiver = {
            let mut queue = self.queue.lock().unwrap();
            if let Some(stream) = queue.ready.remove(&internal_id) {
                trace!("stream already opened");
                return Ok(stream);
            }

            let (sender, receiver) = oneshot::channel();

            // Insert the oneshot into the queue.
            queue.waiting.insert(internal_id, sender);
            // Wake up the connection.
            queue.waker.as_ref().map(|waker| waker.wake_by_ref());

            trace!("waiting for stream");

            receiver
        };

        futures::select! {
            stream = receiver.fuse() =>
                stream
                    .inspect(|_| debug!("caller received stream"))
                    .inspect_err(|_| error!("connection cancelled stream"))
                    .map_err(|_| {
                    std::io::Error::other(format!("connection cancelled stream"))
                }),
            _ = self.close_notify.notified().fuse() => {
                error!("connection closed before stream opened");
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
                let mut stream = client_ctrl.open(b"0").await.unwrap();
                let mut stream2 = client_ctrl.open(b"00").await.unwrap();

                stream.write_all(b"ping").await.unwrap();
                stream2.write_all(b"ping2").await.unwrap();
            },
            async {
                let mut stream = server_ctrl.open(b"0").await.unwrap();
                let mut stream2 = server_ctrl.open(b"00").await.unwrap();

                let mut buf = [0; 4];
                stream.read_exact(&mut buf).await.unwrap();
                assert_eq!(&buf, b"ping");

                let mut buf = [0; 5];
                stream2.read_exact(&mut buf).await.unwrap();
                assert_eq!(&buf, b"ping2");
            }
        );

        client_ctrl.close();
        server_ctrl.close();

        conn_task.await.unwrap();
    }

    #[tokio::test]
    async fn test_yamux_client_close() {
        let (client_io, server_io) = duplex(1024);
        let client = Yamux::new(client_io.compat(), Config::default(), Mode::Client);
        let server = Yamux::new(server_io.compat(), Config::default(), Mode::Server);

        let client_ctrl = client.control();

        let mut fut = futures::future::try_join(client.into_future(), server.into_future());

        _ = futures::poll!(&mut fut);

        client_ctrl.close();

        // Both connections close cleanly.
        fut.await.unwrap();
    }

    // Test the case where the client closes the connection while the server is expecting a new stream.
    #[tokio::test]
    async fn test_yamux_client_close_early() {
        let (client_io, server_io) = duplex(1024);
        let client = Yamux::new(client_io.compat(), Config::default(), Mode::Client);
        let server = Yamux::new(server_io.compat(), Config::default(), Mode::Server);

        let client_ctrl = client.control();
        let server_ctrl = server.control();

        let mut fut_conn = futures::future::try_join(client.into_future(), server.into_future());
        _ = futures::poll!(&mut fut_conn);

        let mut fut_open = server_ctrl.open(b"0");
        _ = futures::poll!(&mut fut_open);

        client_ctrl.close();

        // Both connections close cleanly.
        fut_conn.await.unwrap();
        // But caller gets an error.
        assert!(fut_open.await.is_err());
    }

    #[tokio::test]
    async fn test_yamux_server_close() {
        let (client_io, server_io) = duplex(1024);
        let client = Yamux::new(client_io.compat(), Config::default(), Mode::Client);
        let server = Yamux::new(server_io.compat(), Config::default(), Mode::Server);

        let server_ctrl = server.control();

        let mut fut = futures::future::try_join(client.into_future(), server.into_future());

        _ = futures::poll!(&mut fut);

        server_ctrl.close();

        // Both connections close cleanly.
        fut.await.unwrap();
    }

    // Test the case where the server closes the connection while the client is opening a new stream.
    #[tokio::test]
    async fn test_yamux_server_close_early() {
        let (client_io, server_io) = duplex(1024);
        let client = Yamux::new(client_io.compat(), Config::default(), Mode::Client);
        let server = Yamux::new(server_io.compat(), Config::default(), Mode::Server);

        let client_ctrl = client.control();
        let server_ctrl = server.control();

        let mut fut_client = client.into_future();
        let mut fut_server = server.into_future();

        let mut fut_conn = futures::future::try_join(&mut fut_client, &mut fut_server);
        _ = futures::poll!(&mut fut_conn);
        drop(fut_conn);

        let mut fut_open = client_ctrl.open(b"0");
        _ = futures::poll!(&mut fut_open);

        // We need to prevent the client from beating us to the punch here.
        fut_client.queue.lock().unwrap().waiting.clear();

        server_ctrl.close();

        // Both connections close cleanly.
        futures::try_join!(fut_client, fut_server).unwrap();
        // But caller gets an error.
        assert!(fut_open.await.is_err());
    }
}
