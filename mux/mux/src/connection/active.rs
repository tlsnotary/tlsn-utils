use crate::{
    Config, Result,
    error::ConnectionError,
    frame::{
        self, Frame,
        header::{
            self, CONNECTION_ID, Data, GoAway, Header, Ping, StreamId, StreamInit, Tag,
            WindowUpdate,
        },
    },
    tagged_stream::TaggedStream,
};
use futures::{
    channel::mpsc,
    future::Either,
    prelude::*,
    stream::{Fuse, SelectAll},
};
use nohash_hasher::IntMap;
use parking_lot::Mutex;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt,
    sync::Arc,
    task::{Context, Poll, Waker},
};

type PendingFrames = VecDeque<Frame<()>>;

use super::{
    Id, Mode,
    cleanup::Cleanup,
    closing::Closing,
    rtt,
    stream::{self, State, Stream},
};

/// `Stream` to `Connection` commands.
#[derive(Debug)]
pub(crate) enum StreamCommand {
    /// A new frame should be sent to the remote.
    SendFrame(Frame<Either<Data, WindowUpdate>>),
    /// Send StreamInit frame (client only, on first write).
    SendInit {
        stream_id: StreamId,
        user_id: Vec<u8>,
    },
    /// Close a stream.
    CloseStream { stream_id: StreamId, ack: bool },
}

/// Possible actions as a result of incoming frame handling.
#[derive(Debug)]
pub(crate) enum Action {
    /// Nothing to be done.
    None,
    /// A ping should be answered.
    Ping(Frame<Ping>),
    /// The connection should be terminated.
    Terminate(Frame<GoAway>),
}

/// The active state of [`super::Connection`].
pub(crate) struct Active<T> {
    id: Id,
    mode: Mode,
    pub(super) config: Arc<Config>,
    socket: Fuse<frame::Io<T>>,
    next_id: u32,

    streams: IntMap<StreamId, Arc<Mutex<stream::Shared>>>,
    stream_receivers: SelectAll<TaggedStream<StreamId, mpsc::Receiver<StreamCommand>>>,
    no_streams_waker: Option<Waker>,

    /// Server only: streams pre-registered via new_stream, waiting for client's StreamInit.
    /// Stores shared state and the receiver for command handling.
    pending_streams: HashMap<Vec<u8>, (Arc<Mutex<stream::Shared>>, mpsc::Receiver<StreamCommand>)>,
    /// Server only: StreamInit frames received before server called new_stream.
    buffered_inits: HashMap<Vec<u8>, StreamId>,

    /// Tracks used user-defined stream IDs to ensure uniqueness.
    user_ids: HashSet<Vec<u8>>,

    pending_read_frame: Option<Frame<()>>,
    pending_write_frame: Option<Frame<()>>,

    rtt: rtt::Rtt,

    /// A stream's `max_stream_receive_window` can grow beyond
    /// [`DEFAULT_CREDIT`], see [`Stream::next_window_update`]. This field
    /// is the sum of the bytes by which all streams'
    /// `max_stream_receive_window` have each exceeded [`DEFAULT_CREDIT`]. Used
    /// to enforce [`Config::max_connection_receive_window`].
    accumulated_max_stream_windows: Arc<Mutex<usize>>,
}

impl<T> fmt::Debug for Active<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Connection")
            .field("id", &self.id)
            .field("mode", &self.mode)
            .field("streams", &self.streams.len())
            .field("next_id", &self.next_id)
            .finish()
    }
}

impl<T> fmt::Display for Active<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "(Connection {} {:?} (streams {}))",
            self.id,
            self.mode,
            self.streams.len()
        )
    }
}

impl<T: AsyncRead + AsyncWrite + Unpin> Active<T> {
    /// Create a new `Connection` from the given I/O resource.
    pub(super) fn new(socket: T, cfg: Config, mode: Mode) -> Self {
        let id = Id::random();
        log::debug!("new connection: {id} ({mode:?})");
        let socket = frame::Io::new(id, socket).fuse();
        Active {
            id,
            mode,
            config: Arc::new(cfg),
            socket,
            streams: IntMap::default(),
            stream_receivers: SelectAll::default(),
            no_streams_waker: None,
            pending_streams: HashMap::default(),
            buffered_inits: HashMap::default(),
            user_ids: HashSet::default(),
            next_id: 1,
            pending_read_frame: None,
            pending_write_frame: None,
            rtt: rtt::Rtt::new(),
            accumulated_max_stream_windows: Default::default(),
        }
    }

    /// Gracefully close the connection to the remote.
    pub(super) fn close(self) -> Closing<T> {
        let wait_for_reply = self.config.close_sync;
        let pending_frames = self
            .pending_read_frame
            .into_iter()
            .chain(self.pending_write_frame)
            .collect::<PendingFrames>();
        Closing::new(
            self.id,
            self.stream_receivers,
            pending_frames,
            self.socket,
            wait_for_reply,
            self.config.keep_alive,
        )
    }

    /// Close the connection without waiting for a reply.
    ///
    /// Used when we received a GoAway from remote and need to send our reply,
    /// but don't need to wait since we already got their GoAway.
    pub(super) fn close_no_wait(self) -> Closing<T> {
        let pending_frames = self
            .pending_read_frame
            .into_iter()
            .chain(self.pending_write_frame)
            .collect::<PendingFrames>();
        Closing::new(
            self.id,
            self.stream_receivers,
            pending_frames,
            self.socket,
            false,
            self.config.keep_alive,
        )
    }

    /// Cleanup all our resources.
    ///
    /// This should be called in the context of an unrecoverable error on the
    /// connection.
    pub(super) fn cleanup(mut self, error: ConnectionError) -> Cleanup {
        self.drop_all_streams();

        Cleanup::new(self.stream_receivers, error)
    }

    pub(super) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<Result<()>> {
        loop {
            if self.socket.poll_ready_unpin(cx).is_ready() {
                // Note `next_ping` does not register a waker and thus if not called regularly
                // (idle connection) no ping is sent. This is deliberate as an
                // idle connection does not need RTT measurements to increase
                // its stream receive window.
                if let Some(frame) = self.rtt.next_ping() {
                    self.socket.start_send_unpin(frame.into())?;
                    continue;
                }

                // Privilege pending `Pong` and `GoAway` `Frame`s
                // over `Frame`s from the receivers.
                if let Some(frame) = self
                    .pending_read_frame
                    .take()
                    .or_else(|| self.pending_write_frame.take())
                {
                    self.socket.start_send_unpin(frame)?;
                    continue;
                }
            }

            match self.socket.poll_flush_unpin(cx)? {
                Poll::Ready(()) => {}
                Poll::Pending => {}
            }

            if self.pending_write_frame.is_none() {
                match self.stream_receivers.poll_next_unpin(cx) {
                    Poll::Ready(Some((_, Some(StreamCommand::SendFrame(frame))))) => {
                        log::trace!(
                            "{}/{}: sending: {}",
                            self.id,
                            frame.header().stream_id(),
                            frame.header()
                        );
                        self.pending_write_frame.replace(frame.into());
                        continue;
                    }
                    Poll::Ready(Some((
                        _,
                        Some(StreamCommand::SendInit { stream_id, user_id }),
                    ))) => {
                        log::trace!("{}/{}: sending StreamInit", self.id, stream_id);
                        let frame = Frame::<StreamInit>::stream_init(stream_id, Some(&user_id));
                        self.pending_write_frame.replace(frame.into());
                        continue;
                    }
                    Poll::Ready(Some((_, Some(StreamCommand::CloseStream { stream_id, ack })))) => {
                        log::trace!("{}/{}: sending close", self.id, stream_id);
                        self.pending_write_frame
                            .replace(Frame::close_stream(stream_id, ack).into());
                        continue;
                    }
                    Poll::Ready(Some((id, None))) => {
                        if let Some(frame) = self.on_drop_stream(id) {
                            log::trace!("{}/{}: sending: {}", self.id, id, frame.header());
                            self.pending_write_frame.replace(frame);
                        };
                        continue;
                    }
                    Poll::Ready(None) => {
                        self.no_streams_waker = Some(cx.waker().clone());
                    }
                    Poll::Pending => {}
                }
            }

            if self.pending_read_frame.is_none() {
                match self.socket.poll_next_unpin(cx) {
                    Poll::Ready(Some(frame)) => {
                        match self.on_frame(frame?)? {
                            Action::None => {}
                            Action::Ping(f) => {
                                log::trace!("{}/{}: pong", self.id, f.header().stream_id());
                                self.pending_read_frame.replace(f.into());
                            }
                            Action::Terminate(f) => {
                                log::trace!("{}: sending term", self.id);
                                self.pending_read_frame.replace(f.into());
                            }
                        }
                        continue;
                    }
                    Poll::Ready(None) => {
                        return Poll::Ready(Err(ConnectionError::Closed));
                    }
                    Poll::Pending => {}
                }
            }

            // If we make it this far, at least one of the above must have registered a
            // waker.
            return Poll::Pending;
        }
    }

    /// Create a new stream.
    ///
    /// For client mode: Creates stream in Initializing state, StreamInit will be sent on first write.
    /// For server mode: Pre-registers stream or matches with buffered StreamInit.
    pub(super) fn new_stream(&mut self, user_id: &[u8]) -> Result<Stream> {
        // Validate user ID length (1-32 bytes)
        if user_id.is_empty() || user_id.len() > 32 {
            return Err(ConnectionError::InvalidUserIdLength);
        }

        // Check uniqueness
        if self.user_ids.contains(user_id) {
            return Err(ConnectionError::DuplicateUserId);
        }

        let total_streams = self.streams.len() + self.pending_streams.len();
        if total_streams >= self.config.max_num_streams {
            log::error!("{}: maximum number of streams reached", self.id);
            return Err(ConnectionError::TooManyStreams);
        }

        let user_id_vec = user_id.to_vec();
        self.user_ids.insert(user_id_vec.clone());

        match self.mode {
            Mode::Client => {
                let stream_id = self.next_stream_id()?;
                log::trace!("{}: creating new client stream {}", self.id, stream_id);

                let stream = self.make_client_stream(stream_id, user_id_vec);
                self.streams.insert(stream_id, stream.clone_shared());

                log::debug!("{}: new client stream {} of {}", self.id, stream, self);
                Ok(stream)
            }
            Mode::Server => {
                // Check if we have a buffered StreamInit for this user_id
                if let Some(stream_id) = self.buffered_inits.remove(&user_id_vec) {
                    log::trace!(
                        "{}: matching buffered StreamInit {} for user_id",
                        self.id,
                        stream_id
                    );
                    let stream = self.make_server_matched_stream(stream_id, user_id_vec);
                    self.streams.insert(stream_id, stream.clone_shared());

                    log::debug!(
                        "{}: new matched server stream {} of {}",
                        self.id,
                        stream,
                        self
                    );
                    Ok(stream)
                } else {
                    log::trace!("{}: pre-registering server stream for user_id", self.id);
                    let (stream, receiver) = self.make_server_pending_stream(user_id_vec.clone());
                    self.pending_streams
                        .insert(user_id_vec, (stream.clone_shared(), receiver));

                    log::debug!(
                        "{}: new pending server stream {} of {}",
                        self.id,
                        stream,
                        self
                    );
                    Ok(stream)
                }
            }
        }
    }

    fn on_drop_stream(&mut self, stream_id: StreamId) -> Option<Frame<()>> {
        let s = self.streams.remove(&stream_id).expect("stream not found");

        log::trace!("{}: removing dropped stream {}", self.id, stream_id);
        let frame = {
            let mut shared = s.lock();
            let frame = match shared.update_state(self.id, Some(stream_id), State::Closed) {
                // Stream was in Initializing state - remote doesn't know about it yet.
                // No need to send anything.
                State::Initializing => None,
                // The stream was dropped without calling `poll_close`.
                // We reset the stream to inform the remote of the closure.
                State::Open { .. } => {
                    let mut header = Header::data(stream_id, 0);
                    header.rst();
                    Some(Frame::new(header))
                }
                // The stream was dropped without calling `poll_close`.
                // We have already received a FIN from remote and send one
                // back which closes the stream for good.
                State::RecvClosed => {
                    let mut header = Header::data(stream_id, 0);
                    header.fin();
                    Some(Frame::new(header))
                }
                // The stream was properly closed. We already sent our FIN frame.
                // The remote may be out of credit though and blocked on
                // writing more data. We may need to reset the stream.
                State::SendClosed => {
                    // The remote has either still credit or will be given more
                    // due to an enqueued window update or we already have
                    // inbound frames in the socket buffer which will be
                    // processed later. In any case we will reply with an RST in
                    // `Connection::on_data` because the stream will no longer
                    // be known.
                    None
                }
                // The stream was properly closed. We already have sent our FIN frame. The
                // remote end has already done so in the past.
                State::Closed => None,
            };
            if let Some(w) = shared.reader.take() {
                w.wake()
            }
            if let Some(w) = shared.writer.take() {
                w.wake()
            }
            frame
        };
        frame.map(Into::into)
    }

    /// Process the result of reading from the socket.
    ///
    /// Unless `frame` is `Ok(Some(_))` we will assume the connection got closed
    /// and return a corresponding error, which terminates the connection.
    /// Otherwise we process the frame and potentially return a new `Stream`
    /// if one was opened by the remote.
    fn on_frame(&mut self, frame: Frame<()>) -> Result<Action> {
        log::trace!("{}: received: {}", self.id, frame.header());

        if frame.header().flags().contains(header::ACK)
            && matches!(frame.header().tag(), Tag::Data | Tag::WindowUpdate)
        {
            let id = frame.header().stream_id();
            if let Some(stream) = self.streams.get(&id) {
                stream
                    .lock()
                    .update_state(self.id, Some(id), State::Open { acknowledged: true });
            }
        }

        let action = match frame.header().tag() {
            Tag::Data => self.on_data(frame.into_data()),
            Tag::WindowUpdate => self.on_window_update(&frame.into_window_update()),
            Tag::Ping => self.on_ping(&frame.into_ping()),
            Tag::GoAway => return Err(ConnectionError::Closed),
            Tag::StreamInit => self.on_stream_init(frame.into_stream_init()),
        };
        Ok(action)
    }

    fn on_data(&mut self, frame: Frame<Data>) -> Action {
        let stream_id = frame.header().stream_id();

        if frame.header().flags().contains(header::RST) {
            // stream reset
            if let Some(s) = self.streams.get_mut(&stream_id) {
                let mut shared = s.lock();
                shared.update_state(self.id, Some(stream_id), State::Closed);
                if let Some(w) = shared.reader.take() {
                    w.wake()
                }
                if let Some(w) = shared.writer.take() {
                    w.wake()
                }
            }
            return Action::None;
        }

        let is_finish = frame.header().flags().contains(header::FIN); // half-close

        // SYN flag on Data frames is no longer used for stream initiation.
        // Streams are now initiated via StreamInit frames.
        if frame.header().flags().contains(header::SYN) {
            log::error!("{}: SYN flag on Data frame is not allowed", self.id);
            return Action::Terminate(Frame::protocol_error());
        }

        if let Some(s) = self.streams.get_mut(&stream_id) {
            let mut shared = s.lock();
            if frame.body_len() > shared.receive_window() {
                log::error!(
                    "{}/{}: frame body larger than window of stream",
                    self.id,
                    stream_id
                );
                return Action::Terminate(Frame::protocol_error());
            }
            if is_finish {
                shared.update_state(self.id, Some(stream_id), State::RecvClosed);
            }
            shared.consume_receive_window(frame.body_len());
            shared.buffer.push(frame.into_body());
            if let Some(w) = shared.reader.take() {
                w.wake()
            }
        } else {
            log::trace!(
                "{}/{}: data frame for unknown stream, possibly dropped earlier: {:?}",
                self.id,
                stream_id,
                frame
            );
            // We do not consider this a protocol violation and thus do not send
            // a stream reset because we may still be processing
            // pending `StreamCommand`s of this stream that were
            // sent before it has been dropped and "garbage collected". Such a
            // stream reset would interfere with the frames that
            // still need to be sent, causing premature stream
            // termination for the remote.
            //
            // See https://github.com/paritytech/yamux/issues/110 for details.
        }

        Action::None
    }

    fn on_window_update(&mut self, frame: &Frame<WindowUpdate>) -> Action {
        let stream_id = frame.header().stream_id();

        if frame.header().flags().contains(header::RST) {
            // stream reset
            if let Some(s) = self.streams.get_mut(&stream_id) {
                let mut shared = s.lock();
                shared.update_state(self.id, Some(stream_id), State::Closed);
                if let Some(w) = shared.reader.take() {
                    w.wake()
                }
                if let Some(w) = shared.writer.take() {
                    w.wake()
                }
            }
            return Action::None;
        }

        let is_finish = frame.header().flags().contains(header::FIN); // half-close

        // SYN flag on WindowUpdate frames is no longer used for stream initiation.
        // Streams are now initiated via StreamInit frames.
        if frame.header().flags().contains(header::SYN) {
            log::error!("{}: SYN flag on WindowUpdate frame is not allowed", self.id);
            return Action::Terminate(Frame::protocol_error());
        }

        if let Some(s) = self.streams.get_mut(&stream_id) {
            let mut shared = s.lock();
            shared.increase_send_window_by(frame.header().credit());
            if is_finish {
                shared.update_state(self.id, Some(stream_id), State::RecvClosed);

                if let Some(w) = shared.reader.take() {
                    w.wake()
                }
            }
            if let Some(w) = shared.writer.take() {
                w.wake()
            }
        } else {
            log::trace!(
                "{}/{}: window update for unknown stream, possibly dropped earlier: {:?}",
                self.id,
                stream_id,
                frame
            );
            // We do not consider this a protocol violation and thus do not send
            // a stream reset because we may still be processing
            // pending `StreamCommand`s of this stream that were
            // sent before it has been dropped and "garbage collected". Such a
            // stream reset would interfere with the frames that
            // still need to be sent, causing premature stream
            // termination for the remote.
            //
            // See https://github.com/paritytech/yamux/issues/110 for details.
        }

        Action::None
    }

    fn on_ping(&mut self, frame: &Frame<Ping>) -> Action {
        let stream_id = frame.header().stream_id();
        if frame.header().flags().contains(header::ACK) {
            return self.rtt.handle_pong(frame.nonce());
        }
        if stream_id == CONNECTION_ID || self.streams.contains_key(&stream_id) {
            let mut hdr = Header::ping(frame.header().nonce());
            hdr.ack();
            return Action::Ping(Frame::new(hdr));
        }
        log::debug!(
            "{}/{}: ping for unknown stream, possibly dropped earlier: {:?}",
            self.id,
            stream_id,
            frame
        );
        // We do not consider this a protocol violation and thus do not send a stream
        // reset because we may still be processing pending `StreamCommand`s of
        // this stream that were sent before it has been dropped and "garbage
        // collected". Such a stream reset would interfere with the frames that
        // still need to be sent, causing premature stream termination for the remote.
        //
        // See https://github.com/paritytech/yamux/issues/110 for details.

        Action::None
    }

    fn on_stream_init(&mut self, frame: Frame<StreamInit>) -> Action {
        let stream_id = frame.header().stream_id();

        // Only server can receive StreamInit (only client can open streams)
        if self.mode == Mode::Client {
            log::error!("{}: server cannot send StreamInit frames", self.id);
            return Action::Terminate(Frame::protocol_error());
        }

        if stream_id.is_session() {
            log::error!("{}: invalid stream id 0 for StreamInit", self.id);
            return Action::Terminate(Frame::protocol_error());
        }

        if self.streams.contains_key(&stream_id) {
            log::error!("{}/{}: stream already exists", self.id, stream_id);
            return Action::Terminate(Frame::protocol_error());
        }

        let user_id = frame.into_user_id().unwrap_or_default();

        // Check if server has pre-registered a stream for this user_id
        if let Some((shared, receiver)) = self.pending_streams.remove(&user_id) {
            log::trace!(
                "{}/{}: matching pre-registered stream for user_id",
                self.id,
                stream_id
            );
            // Set stream_id, transition to Open, and wake waiters
            {
                let mut s = shared.lock();
                s.set_stream_id(stream_id);
                s.update_state(
                    self.id,
                    Some(stream_id),
                    State::Open {
                        acknowledged: false,
                    },
                );
                if let Some(w) = s.reader.take() {
                    w.wake();
                }
                if let Some(w) = s.writer.take() {
                    w.wake();
                }
            }
            // Move to streams map
            self.streams.insert(stream_id, shared.clone());
            // Set up command receiver
            self.stream_receivers
                .push(TaggedStream::new(stream_id, receiver));
            if let Some(waker) = self.no_streams_waker.take() {
                waker.wake();
            }
            Action::None
        } else {
            // Only check limit when buffering - matching doesn't increase total count
            let total_streams =
                self.streams.len() + self.pending_streams.len() + self.buffered_inits.len();
            if total_streams >= self.config.max_num_streams {
                log::error!("{}: maximum number of streams reached", self.id);
                return Action::Terminate(Frame::internal_error());
            }

            log::trace!(
                "{}/{}: buffering StreamInit for user_id (no pre-registration)",
                self.id,
                stream_id
            );
            // Buffer for later registration by server
            self.buffered_inits.insert(user_id, stream_id);
            Action::None
        }
    }

    fn make_client_stream(&mut self, id: StreamId, user_id: Vec<u8>) -> Stream {
        let config = self.config.clone();

        let (sender, receiver) = mpsc::channel(10);
        self.stream_receivers.push(TaggedStream::new(id, receiver));
        if let Some(waker) = self.no_streams_waker.take() {
            waker.wake();
        }

        Stream::new_client(
            id,
            user_id,
            self.id,
            config,
            sender,
            self.rtt.clone(),
            self.accumulated_max_stream_windows.clone(),
        )
    }

    fn make_server_pending_stream(
        &mut self,
        user_id: Vec<u8>,
    ) -> (Stream, mpsc::Receiver<StreamCommand>) {
        let config = self.config.clone();

        let (sender, receiver) = mpsc::channel(10);
        // Receiver will be pushed to stream_receivers when matched

        let stream = Stream::new_server_pending(
            user_id,
            self.id,
            config,
            sender,
            self.rtt.clone(),
            self.accumulated_max_stream_windows.clone(),
        );
        (stream, receiver)
    }

    fn make_server_matched_stream(&mut self, id: StreamId, user_id: Vec<u8>) -> Stream {
        let config = self.config.clone();

        let (sender, receiver) = mpsc::channel(10);
        self.stream_receivers.push(TaggedStream::new(id, receiver));
        if let Some(waker) = self.no_streams_waker.take() {
            waker.wake();
        }

        Stream::new_server_matched(
            id,
            user_id,
            self.id,
            config,
            sender,
            self.rtt.clone(),
            self.accumulated_max_stream_windows.clone(),
        )
    }

    fn next_stream_id(&mut self) -> Result<StreamId> {
        let proposed = StreamId::new(self.next_id);
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(ConnectionError::NoMoreStreamIds)?;
        Ok(proposed)
    }
}

impl<T> Active<T> {
    /// Close and drop all `Stream`s and wake any pending `Waker`s.
    pub(super) fn drop_all_streams(&mut self) {
        for (id, s) in self.streams.drain() {
            let mut shared = s.lock();
            shared.update_state(self.id, Some(id), State::Closed);
            if let Some(w) = shared.reader.take() {
                w.wake()
            }
            if let Some(w) = shared.writer.take() {
                w.wake()
            }
        }
        // Also close pending streams
        for (_, (s, _receiver)) in self.pending_streams.drain() {
            let mut shared = s.lock();
            shared.update_state(self.id, None, State::Closed);
            if let Some(w) = shared.reader.take() {
                w.wake()
            }
            if let Some(w) = shared.writer.take() {
                w.wake()
            }
        }
    }
}
