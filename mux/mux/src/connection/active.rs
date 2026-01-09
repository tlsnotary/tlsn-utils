use crate::{
    Config, DEFAULT_CREDIT, MAX_ACK_BACKLOG, Result,
    error::ConnectionError,
    frame::{
        self, Frame,
        header::{self, CONNECTION_ID, Data, GoAway, Header, Ping, StreamId, Tag, WindowUpdate},
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
    collections::VecDeque,
    fmt,
    sync::Arc,
    task::{Context, Poll, Waker},
};

use super::{Id, Mode, cleanup::Cleanup, closing::Closing, rtt, stream::{self, State, Stream}};

/// `Stream` to `Connection` commands.
#[derive(Debug)]
pub(crate) enum StreamCommand {
    /// A new frame should be sent to the remote.
    SendFrame(Frame<Either<Data, WindowUpdate>>),
    /// Close a stream.
    CloseStream { ack: bool },
}

/// Possible actions as a result of incoming frame handling.
#[derive(Debug)]
pub(crate) enum Action {
    /// Nothing to be done.
    None,
    /// A new stream has been opened by the remote.
    New(Stream),
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

    pending_read_frame: Option<Frame<()>>,
    pending_write_frame: Option<Frame<()>>,
    new_outbound_stream_waker: Option<Waker>,

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
            next_id: match mode {
                Mode::Client => 1,
                Mode::Server => 2,
            },
            pending_read_frame: None,
            pending_write_frame: None,
            new_outbound_stream_waker: None,
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
            .collect::<VecDeque<Frame<()>>>();
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
            .collect::<VecDeque<Frame<()>>>();
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

    pub(super) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<Result<Stream>> {
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
                    Poll::Ready(Some((id, Some(StreamCommand::CloseStream { ack })))) => {
                        log::trace!("{}/{}: sending close", self.id, id);
                        self.pending_write_frame
                            .replace(Frame::close_stream(id, ack).into());
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
                            Action::New(stream) => {
                                log::trace!("{}: new inbound {} of {}", self.id, stream, self);
                                return Poll::Ready(Ok(stream));
                            }
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

    pub(super) fn poll_new_outbound(&mut self, cx: &mut Context<'_>) -> Poll<Result<Stream>> {
        if self.streams.len() >= self.config.max_num_streams {
            log::error!("{}: maximum number of streams reached", self.id);
            return Poll::Ready(Err(ConnectionError::TooManyStreams));
        }

        if self.ack_backlog() >= MAX_ACK_BACKLOG {
            log::debug!(
                "{MAX_ACK_BACKLOG} streams waiting for ACK, registering task for wake-up until remote acknowledges at least one stream"
            );
            self.new_outbound_stream_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }

        log::trace!("{}: creating new outbound stream", self.id);

        let id = self.next_stream_id()?;
        let stream = self.make_new_outbound_stream(id);

        log::debug!("{}: new outbound {} of {}", self.id, stream, self);
        self.streams.insert(id, stream.clone_shared());

        Poll::Ready(Ok(stream))
    }

    fn on_drop_stream(&mut self, stream_id: StreamId) -> Option<Frame<()>> {
        let s = self.streams.remove(&stream_id).expect("stream not found");

        log::trace!("{}: removing dropped stream {}", self.id, stream_id);
        let frame = {
            let mut shared = s.lock();
            let frame = match shared.update_state(self.id, stream_id, State::Closed) {
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
                    .update_state(self.id, id, State::Open { acknowledged: true });
            }
            if let Some(waker) = self.new_outbound_stream_waker.take() {
                waker.wake();
            }
        }

        let action = match frame.header().tag() {
            Tag::Data => self.on_data(frame.into_data()),
            Tag::WindowUpdate => self.on_window_update(&frame.into_window_update()),
            Tag::Ping => self.on_ping(&frame.into_ping()),
            Tag::GoAway => return Err(ConnectionError::Closed),
        };
        Ok(action)
    }

    fn on_data(&mut self, frame: Frame<Data>) -> Action {
        let stream_id = frame.header().stream_id();

        if frame.header().flags().contains(header::RST) {
            // stream reset
            if let Some(s) = self.streams.get_mut(&stream_id) {
                let mut shared = s.lock();
                shared.update_state(self.id, stream_id, State::Closed);
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

        if frame.header().flags().contains(header::SYN) {
            // new stream
            if !self.is_valid_remote_id(stream_id, Tag::Data) {
                log::error!("{}: invalid stream id {}", self.id, stream_id);
                return Action::Terminate(Frame::protocol_error());
            }
            if frame.body().len() > DEFAULT_CREDIT as usize {
                log::error!(
                    "{}/{}: 1st body of stream exceeds default credit",
                    self.id,
                    stream_id
                );
                return Action::Terminate(Frame::protocol_error());
            }
            if self.streams.contains_key(&stream_id) {
                log::error!("{}/{}: stream already exists", self.id, stream_id);
                return Action::Terminate(Frame::protocol_error());
            }
            if self.streams.len() == self.config.max_num_streams {
                log::error!("{}: maximum number of streams reached", self.id);
                return Action::Terminate(Frame::internal_error());
            }
            let stream = self.make_new_inbound_stream(stream_id, DEFAULT_CREDIT);
            {
                let mut shared = stream.shared();
                if is_finish {
                    shared.update_state(self.id, stream_id, State::RecvClosed);
                }
                shared.consume_receive_window(frame.body_len());
                shared.buffer.push(frame.into_body());
            }
            self.streams.insert(stream_id, stream.clone_shared());
            return Action::New(stream);
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
                shared.update_state(self.id, stream_id, State::RecvClosed);
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
                shared.update_state(self.id, stream_id, State::Closed);
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

        if frame.header().flags().contains(header::SYN) {
            // new stream
            if !self.is_valid_remote_id(stream_id, Tag::WindowUpdate) {
                log::error!("{}: invalid stream id {}", self.id, stream_id);
                return Action::Terminate(Frame::protocol_error());
            }
            if self.streams.contains_key(&stream_id) {
                log::error!("{}/{}: stream already exists", self.id, stream_id);
                return Action::Terminate(Frame::protocol_error());
            }
            if self.streams.len() == self.config.max_num_streams {
                log::error!("{}: maximum number of streams reached", self.id);
                return Action::Terminate(Frame::internal_error());
            }

            let credit = frame.header().credit() + DEFAULT_CREDIT;
            let stream = self.make_new_inbound_stream(stream_id, credit);

            if is_finish {
                stream
                    .shared()
                    .update_state(self.id, stream_id, State::RecvClosed);
            }
            self.streams.insert(stream_id, stream.clone_shared());
            return Action::New(stream);
        }

        if let Some(s) = self.streams.get_mut(&stream_id) {
            let mut shared = s.lock();
            shared.increase_send_window_by(frame.header().credit());
            if is_finish {
                shared.update_state(self.id, stream_id, State::RecvClosed);

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

    fn make_new_inbound_stream(&mut self, id: StreamId, credit: u32) -> Stream {
        let config = self.config.clone();

        let (sender, receiver) = mpsc::channel(10); // 10 is an arbitrary number.
        self.stream_receivers.push(TaggedStream::new(id, receiver));
        if let Some(waker) = self.no_streams_waker.take() {
            waker.wake();
        }

        Stream::new_inbound(
            id,
            self.id,
            config,
            credit,
            sender,
            self.rtt.clone(),
            self.accumulated_max_stream_windows.clone(),
        )
    }

    fn make_new_outbound_stream(&mut self, id: StreamId) -> Stream {
        let config = self.config.clone();

        let (sender, receiver) = mpsc::channel(10); // 10 is an arbitrary number.
        self.stream_receivers.push(TaggedStream::new(id, receiver));
        if let Some(waker) = self.no_streams_waker.take() {
            waker.wake();
        }

        Stream::new_outbound(
            id,
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
            .checked_add(2)
            .ok_or(ConnectionError::NoMoreStreamIds)?;
        match self.mode {
            Mode::Client => assert!(proposed.is_client()),
            Mode::Server => assert!(proposed.is_server()),
        }
        Ok(proposed)
    }

    /// The ACK backlog is defined as the number of outbound streams that have
    /// not yet been acknowledged.
    fn ack_backlog(&mut self) -> usize {
        self.streams
            .iter()
            // Whether this is an outbound stream.
            //
            // Clients use odd IDs and servers use even IDs.
            // A stream is outbound if:
            //
            // - Its ID is odd and we are the client.
            // - Its ID is even and we are the server.
            .filter(|(id, _)| match self.mode {
                Mode::Client => id.is_client(),
                Mode::Server => id.is_server(),
            })
            .filter(|(_, s)| s.lock().is_pending_ack())
            .count()
    }

    // Check if the given stream ID is valid w.r.t. the provided tag and our
    // connection mode.
    fn is_valid_remote_id(&self, id: StreamId, tag: Tag) -> bool {
        if tag == Tag::Ping || tag == Tag::GoAway {
            return id.is_session();
        }
        match self.mode {
            Mode::Client => id.is_server(),
            Mode::Server => id.is_client(),
        }
    }
}

impl<T> Active<T> {
    /// Close and drop all `Stream`s and wake any pending `Waker`s.
    pub(super) fn drop_all_streams(&mut self) {
        for (id, s) in self.streams.drain() {
            let mut shared = s.lock();
            shared.update_state(self.id, id, State::Closed);
            if let Some(w) = shared.reader.take() {
                w.wake()
            }
            if let Some(w) = shared.writer.take() {
                w.wake()
            }
        }
    }
}
