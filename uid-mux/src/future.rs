use std::{
    pin::{pin, Pin},
    task::{Context, Poll},
};

use futures::{ready, AsyncRead, AsyncWrite, Future};
use tokio::sync::oneshot;

use crate::{
    log::{error, trace},
    InternalId,
};

const BUF: usize = 32;

#[derive(Debug)]
struct Inner<Io> {
    io: Io,
    count: u8,
    id: [u8; BUF],
}

impl<Io> Inner<Io> {
    fn is_done(&self) -> bool {
        self.count == 32
    }
}

#[derive(Debug)]
enum State<Io> {
    Pending(Inner<Io>),
    Error,
}

impl<Io> State<Io> {
    fn take(&mut self) -> Self {
        std::mem::replace(self, Self::Error)
    }
}

/// A future that resolves when an id has been read.
#[derive(Debug)]
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub(crate) struct ReadId<Io>(State<Io>);

impl<Io> ReadId<Io> {
    /// Create a new `ReadId` future.
    pub(crate) fn new(io: Io) -> Self {
        Self(State::Pending(Inner {
            io,
            count: 0,
            id: [0u8; BUF],
        }))
    }
}

impl<Io> Future for ReadId<Io>
where
    Io: AsyncRead + Unpin,
{
    type Output = Result<(InternalId, Io), std::io::Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let State::Pending(mut state) = self.0.take() else {
            panic!("poll after completion");
        };

        while let Poll::Ready(read) =
            pin!(&mut state.io).poll_read(cx, &mut state.id[state.count as usize..])?
        {
            state.count += read as u8;
            if state.is_done() {
                let id = InternalId(state.id);
                trace!("read id: {}", id);
                return Poll::Ready(Ok((id, state.io)));
            } else if read == 0 {
                error!("remote closed before sending id");
                return Poll::Ready(Err(std::io::ErrorKind::UnexpectedEof.into()));
            }
        }

        self.0 = State::Pending(state);
        Poll::Pending
    }
}

/// A future that resolves when an id has been written.
#[derive(Debug)]
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub(crate) struct WriteId<Io>(State<Io>);

impl<Io> WriteId<Io> {
    /// Create a new `WriteId` future.
    pub(crate) fn new(io: Io, id: InternalId) -> Self {
        Self(State::Pending(Inner {
            io,
            count: 0,
            id: id.0,
        }))
    }
}

impl<Io> Future for WriteId<Io>
where
    Io: AsyncWrite + Unpin,
{
    type Output = Result<Io, std::io::Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let State::Pending(mut state) = self.0.take() else {
            panic!("poll after completion");
        };

        // If we haven't finished sending the id, keep sending it.
        if !state.is_done() {
            while let Poll::Ready(sent) =
                pin!(&mut state.io).poll_write(cx, &state.id[state.count as usize..])?
            {
                state.count += sent as u8;
                if state.is_done() {
                    break;
                }
            }
        }

        // If we've finished sending, flush the write buffer. If flushing
        // succeeds then we can return Ready, otherwise we need to keep
        // trying.
        if state.is_done() {
            if pin!(&mut state.io).poll_flush(cx)?.is_ready() {
                return Poll::Ready(Ok(state.io));
            }
        }

        self.0 = State::Pending(state);

        Poll::Pending
    }
}

/// A future that resolves when a stream has been returned to the caller.
#[derive(Debug)]
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub(crate) struct ReturnStream<Io> {
    fut: WriteId<Io>,
    sender: Option<oneshot::Sender<Io>>,
}

impl<Io> ReturnStream<Io> {
    /// Create a new `ReturnStream` future.
    pub(crate) fn new(id: InternalId, io: Io, sender: oneshot::Sender<Io>) -> Self {
        Self {
            fut: WriteId::new(io, id),
            sender: Some(sender),
        }
    }
}

impl<Io> Future for ReturnStream<Io>
where
    Io: AsyncWrite + Unpin,
{
    type Output = Result<(), std::io::Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let io = ready!(pin!(&mut self.fut).poll(cx))?;

        _ = self
            .sender
            .take()
            .expect("future not polled after completion")
            .send(io);

        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tokio::io::duplex;
    use tokio_util::compat::TokioAsyncReadCompatExt as _;

    #[test]
    fn test_id_future() {
        tracing_subscriber::fmt::init();
        let id_0 = InternalId([42u8; 32]);

        // send 1 byte at a time
        let (io_0, io_1) = duplex(1);

        futures::executor::block_on(async {
            let (_, (id_1, _)) = futures::try_join!(
                WriteId::new(io_0.compat(), id_0),
                ReadId::new(io_1.compat())
            )
            .unwrap();

            assert_eq!(id_0, id_1);
        });
    }
}
