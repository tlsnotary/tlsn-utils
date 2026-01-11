// Copyright (c) 2018-2019 Parity Technologies (UK) Ltd.
// Modifications Copyright (c) 2026 TLSNotary
//
// Licensed under the Apache License, Version 2.0 or MIT license, at your
// option.
//
// A copy of the Apache License, Version 2.0 is included in the software as
// LICENSE-APACHE and a copy of the MIT license is included in the software
// as LICENSE-MIT. You may also obtain a copy of the Apache License, Version 2.0
// at https://www.apache.org/licenses/LICENSE-2.0 and a copy of the MIT license
// at https://opensource.org/licenses/MIT.

use crate::{
    Config, DEFAULT_CREDIT, Mode,
    chunks::Chunks,
    connection::{self, StreamCommand, UserId, rtt, rtt::Rtt},
    frame::{
        Frame,
        header::{ACK, Data, Header, StreamId, WindowUpdate},
    },
};
use flow_control::FlowController;
use futures::{
    SinkExt,
    channel::mpsc,
    future::Either,
    io::{AsyncRead, AsyncWrite},
    ready,
};
use parking_lot::{Mutex, MutexGuard};
use std::{
    fmt, io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, Waker},
};

mod flow_control;

/// The state of a stream.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Stream is initializing.
    ///
    /// For client streams: StreamInit has not been sent yet.
    /// For server streams: Waiting for client's StreamInit to arrive.
    Initializing,
    /// Open bidirectionally.
    Open {
        /// Whether the stream is acknowledged.
        ///
        /// For outbound streams, this tracks whether the remote has
        /// acknowledged our stream. For inbound streams, this tracks
        /// whether we have acknowledged the stream to the remote.
        ///
        /// This starts out with `false` and is set to `true` when we receive or
        /// send an `ACK` flag for this stream. We may also directly
        /// transition:
        /// - from `Open` to `RecvClosed` if the remote immediately sends `FIN`.
        /// - from `Open` to `Closed` if the remote immediately sends `RST`.
        acknowledged: bool,
    },
    /// Open for incoming messages.
    SendClosed,
    /// Open for outgoing messages.
    RecvClosed,
    /// Closed (terminal state).
    Closed,
}

impl State {
    fn is_initializing(&self) -> bool {
        matches!(self, State::Initializing)
    }

    /// Can we receive messages over this stream?
    pub fn can_read(self) -> bool {
        // Can't read if initializing (server waiting for StreamInit) or closed
        !matches!(
            self,
            State::Initializing | State::RecvClosed | State::Closed
        )
    }

    /// Can we send messages over this stream?
    pub fn can_write(self) -> bool {
        // Initializing is allowed for client (triggers StreamInit send)
        !matches!(self, State::SendClosed | State::Closed)
    }
}

/// Indicate if a flag still needs to be set on an outbound header.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Flag {
    /// No flag needs to be set.
    None,
    /// The stream still needs acknowledgement, so set the ACK flag.
    Ack,
}

/// A multiplexed stream.
///
/// Streams are created via [`crate::Connection::new_stream`].
///
/// `Stream` implements [`AsyncRead`] and [`AsyncWrite`] and also
/// [`futures::stream::Stream`].
pub struct Stream {
    user_id: UserId,
    conn: connection::Id,
    config: Arc<Config>,
    mode: Mode,
    sender: mpsc::Sender<StreamCommand>,
    flag: Flag,
    shared: Arc<Mutex<Shared>>,
}

impl fmt::Debug for Stream {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Stream")
            .field("stream_id", &self.stream_id().map(|id| id.val()))
            .field("user_id", &self.user_id)
            .field("connection", &self.conn)
            .finish()
    }
}

impl fmt::Display for Stream {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.stream_id() {
            Some(id) => write!(f, "(Stream {}/{})", self.conn, id.val()),
            None => write!(f, "(Stream {}/pending)", self.conn),
        }
    }
}

impl Stream {
    /// Create a new stream for client mode (outbound, will send StreamInit on
    /// first write).
    pub(crate) fn new_client(
        stream_id: StreamId,
        user_id: UserId,
        conn: connection::Id,
        config: Arc<Config>,
        sender: mpsc::Sender<StreamCommand>,
        rtt: rtt::Rtt,
        accumulated_max_stream_windows: Arc<Mutex<usize>>,
    ) -> Self {
        Self {
            user_id,
            conn,
            config: config.clone(),
            mode: Mode::Client,
            sender,
            flag: Flag::None,
            shared: Arc::new(Mutex::new(Shared::new(
                Some(stream_id),
                State::Initializing,
                DEFAULT_CREDIT,
                DEFAULT_CREDIT,
                accumulated_max_stream_windows,
                rtt,
                config,
            ))),
        }
    }

    /// Create a new stream for server mode (pending, waiting for StreamInit).
    pub(crate) fn new_server_pending(
        user_id: UserId,
        conn: connection::Id,
        config: Arc<Config>,
        sender: mpsc::Sender<StreamCommand>,
        rtt: rtt::Rtt,
        accumulated_max_stream_windows: Arc<Mutex<usize>>,
    ) -> Self {
        Self {
            user_id,
            conn,
            config: config.clone(),
            mode: Mode::Server,
            sender,
            flag: Flag::Ack,
            shared: Arc::new(Mutex::new(Shared::new(
                None,
                State::Initializing,
                DEFAULT_CREDIT,
                DEFAULT_CREDIT,
                accumulated_max_stream_windows,
                rtt,
                config,
            ))),
        }
    }

    /// Create a new stream for server mode (matched, StreamInit already
    /// received).
    pub(crate) fn new_server_matched(
        stream_id: StreamId,
        user_id: UserId,
        conn: connection::Id,
        config: Arc<Config>,
        sender: mpsc::Sender<StreamCommand>,
        rtt: rtt::Rtt,
        accumulated_max_stream_windows: Arc<Mutex<usize>>,
    ) -> Self {
        Self {
            user_id,
            conn,
            config: config.clone(),
            mode: Mode::Server,
            sender,
            flag: Flag::Ack,
            shared: Arc::new(Mutex::new(Shared::new(
                Some(stream_id),
                State::Open {
                    acknowledged: false,
                },
                DEFAULT_CREDIT,
                DEFAULT_CREDIT,
                accumulated_max_stream_windows,
                rtt,
                config,
            ))),
        }
    }

    /// Get this stream's user-defined identifier.
    pub fn id(&self) -> &[u8] {
        self.user_id.as_bytes()
    }

    /// Get the stream ID. Returns None for server streams that haven't been
    /// matched yet.
    pub(crate) fn stream_id(&self) -> Option<StreamId> {
        self.shared.lock().stream_id()
    }

    pub fn is_write_closed(&self) -> bool {
        matches!(self.shared().state(), State::SendClosed)
    }

    pub fn is_closed(&self) -> bool {
        matches!(self.shared().state(), State::Closed)
    }

    /// Whether we are still waiting for the remote to acknowledge this stream.
    pub fn is_pending_ack(&self) -> bool {
        self.shared().is_pending_ack()
    }

    pub(crate) fn shared(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock()
    }

    /// Send StreamInit command for client streams.
    ///
    /// This is called on first read or write to initialize the stream.
    /// Returns Poll::Ready(Ok(())) when init is sent, Poll::Pending if waiting.
    fn poll_send_stream_init(&mut self, cx: &mut Context) -> Poll<io::Result<()>> {
        debug_assert!(
            self.mode == Mode::Client,
            "only client streams send StreamInit"
        );

        ready!(
            self.sender
                .poll_ready(cx)
                .map_err(|_| self.write_zero_err())?
        );

        let stream_id = self
            .shared()
            .stream_id
            .expect("client stream ID is always set");

        let cmd = StreamCommand::SendInit {
            stream_id,
            user_id: self.user_id.clone(),
        };

        self.sender
            .start_send(cmd)
            .map_err(|_| self.write_zero_err())?;

        self.shared().update_state(
            self.conn,
            Some(stream_id),
            State::Open {
                acknowledged: false,
            },
        );

        Poll::Ready(Ok(()))
    }

    pub(crate) fn clone_shared(&self) -> Arc<Mutex<Shared>> {
        self.shared.clone()
    }

    fn write_zero_err(&self) -> io::Error {
        let msg = format!("{}: connection is closed", self);
        io::Error::new(io::ErrorKind::WriteZero, msg)
    }

    /// Set ACK flag if necessary.
    fn add_flag(&mut self, header: &mut Header<Either<Data, WindowUpdate>>) {
        match self.flag {
            Flag::None => (),
            Flag::Ack => {
                header.ack();
                self.flag = Flag::None
            }
        }
    }

    /// Send new credit to the sending side via a window update message if
    /// permitted.
    fn send_window_update(&mut self, cx: &mut Context) -> Poll<io::Result<()>> {
        if !self.shared.lock().state.can_read() {
            return Poll::Ready(Ok(()));
        }

        ready!(
            self.sender
                .poll_ready(cx)
                .map_err(|_| self.write_zero_err())?
        );

        let mut shared = self.shared.lock();
        let stream_id = shared
            .stream_id()
            .expect("stream ID is set if ready to read");
        let Some(credit) = shared.next_window_update() else {
            return Poll::Ready(Ok(()));
        };
        drop(shared);

        let mut frame = Frame::window_update(stream_id, credit).right();
        self.add_flag(frame.header_mut());
        let cmd = StreamCommand::SendFrame(frame);
        self.sender
            .start_send(cmd)
            .map_err(|_| self.write_zero_err())?;

        Poll::Ready(Ok(()))
    }
}

// Like the `futures::stream::Stream` impl above, but copies bytes into the
// provided mutable slice.
impl AsyncRead for Stream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        // Try to send window update, but don't fail if sender is closed -
        // we may still have buffered data to read.
        match self.send_window_update(cx) {
            Poll::Ready(Ok(())) => {}
            Poll::Ready(Err(_)) if self.sender.is_closed() => {
                // Sender closed, but continue to read buffered data
            }
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            // Continue reading buffered data even though sending a window update blocked.
            Poll::Pending => {}
        }

        // If stream is still initializing, handle based on mode
        if self.shared().state().is_initializing() {
            match self.mode {
                Mode::Client => {
                    // Client sends StreamInit on first read or write
                    ready!(self.poll_send_stream_init(cx)?);
                }
                Mode::Server => {
                    // Server waits for client's StreamInit
                    log::trace!("{}: waiting for stream match", self);
                    self.shared().reader = Some(cx.waker().clone());
                    return Poll::Pending;
                }
            }
        }

        let mut shared = self.shared();

        // Copy data from stream buffer.
        let mut n = 0;
        while let Some(chunk) = shared.buffer.front_mut() {
            if chunk.is_empty() {
                shared.buffer.pop();
                continue;
            }
            let k = std::cmp::min(chunk.len(), buf.len() - n);
            buf[n..n + k].copy_from_slice(&chunk.as_ref()[..k]);
            n += k;
            chunk.advance(k);
            if n == buf.len() {
                break;
            }
        }

        if n > 0 {
            log::trace!("{}: read {} bytes", self, n);
            return Poll::Ready(Ok(n));
        }

        // Buffer is empty, check if sender is closed (connection closed).
        if !self.config.read_after_close && self.sender.is_closed() {
            return Poll::Ready(Ok(0));
        }

        // Buffer is empty, let's check if we can expect to read more data.
        if !shared.state().can_read() {
            log::debug!("{}: eof", self);
            return Poll::Ready(Ok(0)); // stream has been reset
        }

        // Since we have no more data at this point, we want to be woken up
        // by the connection when more becomes available for us.
        shared.reader = Some(cx.waker().clone());

        Poll::Pending
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        // If stream is still initializing, handle based on mode
        if self.shared().state().is_initializing() {
            match self.mode {
                Mode::Client => {
                    // Client sends StreamInit on first read or write
                    ready!(self.poll_send_stream_init(cx)?);
                }
                Mode::Server => {
                    // Server waits for client's StreamInit
                    self.shared().writer = Some(cx.waker().clone());
                    return Poll::Pending;
                }
            }
        }

        // Ensure sender is ready before sending data
        ready!(
            self.sender
                .poll_ready(cx)
                .map_err(|_| self.write_zero_err())?
        );

        let stream_id = self
            .shared()
            .stream_id
            .expect("stream ID should be set after init");
        let body = {
            let mut shared = self.shared();
            if !shared.state().can_write() {
                log::debug!("{}: can no longer write", self);
                return Poll::Ready(Err(self.write_zero_err()));
            }
            if shared.send_window() == 0 {
                log::trace!("{}: no more credit left", self);
                shared.writer = Some(cx.waker().clone());
                return Poll::Pending;
            }
            let k = std::cmp::min(
                shared.send_window(),
                buf.len().try_into().unwrap_or(u32::MAX),
            );
            let k = std::cmp::min(
                k,
                self.config.split_send_size.try_into().unwrap_or(u32::MAX),
            );
            shared.consume_send_window(k);
            Vec::from(&buf[..k as usize])
        };
        let n = body.len();
        let mut frame = Frame::data(stream_id, body)
            .expect("body <= u32::MAX")
            .left();
        self.add_flag(frame.header_mut());
        log::trace!("{}: write {} bytes", self, n);

        // technically, the frame hasn't been sent yet on the wire but from the
        // perspective of this data structure, we've queued the frame for sending
        // We are tracking this information:
        // a) to be consistent with outbound streams
        // b) to correctly test our behaviour around timing of when ACKs are sent. See
        // `ack_timing.rs` test.
        if frame.header().flags().contains(ACK) {
            self.shared().update_state(
                self.conn,
                Some(stream_id),
                State::Open { acknowledged: true },
            );
        }

        let cmd = StreamCommand::SendFrame(frame);
        self.sender
            .start_send(cmd)
            .map_err(|_| self.write_zero_err())?;
        Poll::Ready(Ok(n))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        self.sender
            .poll_flush_unpin(cx)
            .map_err(|_| self.write_zero_err())
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        if self.is_closed() {
            return Poll::Ready(Ok(()));
        }

        // Can't close without stream_id (unmatched server stream)
        let Some(stream_id) = self.stream_id() else {
            // Just mark as closed locally
            self.shared().update_state(self.conn, None, State::Closed);
            return Poll::Ready(Ok(()));
        };

        ready!(
            self.sender
                .poll_ready(cx)
                .map_err(|_| self.write_zero_err())?
        );
        let ack = if self.flag == Flag::Ack {
            self.flag = Flag::None;
            true
        } else {
            false
        };
        log::trace!("{}: close", self);
        let cmd = StreamCommand::CloseStream { stream_id, ack };
        self.sender
            .start_send(cmd)
            .map_err(|_| self.write_zero_err())?;
        self.shared()
            .update_state(self.conn, Some(stream_id), State::SendClosed);
        Poll::Ready(Ok(()))
    }
}

#[derive(Debug)]
pub(crate) struct Shared {
    stream_id: Option<StreamId>,
    state: State,
    flow_controller: FlowController,
    pub(crate) buffer: Chunks,
    pub(crate) reader: Option<Waker>,
    pub(crate) writer: Option<Waker>,
}

impl Shared {
    fn new(
        stream_id: Option<StreamId>,
        initial_state: State,
        receive_window: u32,
        send_window: u32,
        accumulated_max_stream_windows: Arc<Mutex<usize>>,
        rtt: Rtt,
        config: Arc<Config>,
    ) -> Self {
        Shared {
            stream_id,
            state: initial_state,
            flow_controller: FlowController::new(
                receive_window,
                send_window,
                accumulated_max_stream_windows,
                rtt,
                config,
            ),
            buffer: Chunks::new(),
            reader: None,
            writer: None,
        }
    }

    pub(crate) fn stream_id(&self) -> Option<StreamId> {
        self.stream_id
    }

    pub(crate) fn set_stream_id(&mut self, id: StreamId) {
        self.stream_id = Some(id);
    }

    pub(crate) fn state(&self) -> State {
        self.state
    }

    /// Update the stream state and return the state before it was updated.
    pub(crate) fn update_state(
        &mut self,
        cid: connection::Id,
        sid: Option<StreamId>,
        next: State,
    ) -> State {
        use self::State::*;

        let current = self.state;

        match (current, next) {
            (Closed, _) => {}
            (Initializing, Open { .. }) => self.state = next,
            (Initializing, Closed) => self.state = Closed,
            (Initializing, _) => {} // Can only go to Open or Closed from Initializing
            (Open { .. }, _) => self.state = next,
            (RecvClosed, Closed) => self.state = Closed,
            (RecvClosed, Open { .. } | Initializing) => {}
            (RecvClosed, RecvClosed) => {}
            (RecvClosed, SendClosed) => self.state = Closed,
            (SendClosed, Closed) => self.state = Closed,
            (SendClosed, Open { .. } | Initializing) => {}
            (SendClosed, RecvClosed) => self.state = Closed,
            (SendClosed, SendClosed) => {}
        }

        let sid_str = sid
            .map(|id| id.val())
            .map(|v| v.to_string())
            .unwrap_or_else(|| "pending".to_string());
        log::trace!(
            "{}/{}: update state: (from {:?} to {:?} -> {:?})",
            cid,
            sid_str,
            current,
            next,
            self.state
        );

        current // Return the previous stream state for informational purposes.
    }

    pub(crate) fn next_window_update(&mut self) -> Option<u32> {
        self.flow_controller.next_window_update(self.buffer.len())
    }

    /// Whether we are still waiting for the remote to acknowledge this stream.
    pub fn is_pending_ack(&self) -> bool {
        matches!(
            self.state(),
            State::Initializing
                | State::Open {
                    acknowledged: false
                }
        )
    }

    pub(crate) fn send_window(&self) -> u32 {
        self.flow_controller.send_window()
    }

    pub(crate) fn consume_send_window(&mut self, i: u32) {
        self.flow_controller.consume_send_window(i)
    }

    pub(crate) fn increase_send_window_by(&mut self, i: u32) {
        self.flow_controller.increase_send_window_by(i)
    }

    pub(crate) fn receive_window(&self) -> u32 {
        self.flow_controller.receive_window()
    }

    pub(crate) fn consume_receive_window(&mut self, i: u32) {
        self.flow_controller.consume_receive_window(i)
    }
}
