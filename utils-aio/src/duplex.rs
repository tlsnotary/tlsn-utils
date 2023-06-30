use std::{
    io,
    io::{Error, ErrorKind},
    pin::Pin,
    task::{Context, Poll},
};

use futures::{
    channel::mpsc, future::FusedFuture, stream::FusedStream, AsyncRead, AsyncWrite, Future, Sink,
    Stream, TryStream, TryStreamExt,
};

pub trait DuplexByteStream: AsyncWrite + AsyncRead + Unpin {}

impl<T> DuplexByteStream for T where T: AsyncWrite + AsyncRead + Unpin {}

/// Future for the [`expect_next`](Duplex::expect_next) method.
#[derive(Debug)]
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct ExpectNext<'a, St: ?Sized> {
    stream: &'a mut St,
}

impl<St: ?Sized + Unpin> Unpin for ExpectNext<'_, St> {}

impl<'a, St: ?Sized + TryStream + Unpin> ExpectNext<'a, St> {
    pub(super) fn new(stream: &'a mut St) -> Self {
        Self { stream }
    }
}

impl<St: ?Sized + TryStream + Unpin + FusedStream> FusedFuture for ExpectNext<'_, St>
where
    <St as TryStream>::Error: Into<io::Error>,
{
    fn is_terminated(&self) -> bool {
        self.stream.is_terminated()
    }
}

impl<St: ?Sized + TryStream + Unpin> Future for ExpectNext<'_, St>
where
    <St as TryStream>::Error: Into<io::Error>,
{
    type Output = io::Result<St::Ok>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.stream
            .try_poll_next_unpin(cx)
            .map_err(|e| e.into())?
            .map(|item| {
                if let Some(item) = item {
                    Ok(item)
                } else {
                    Err(io::ErrorKind::UnexpectedEof.into())
                }
            })
    }
}

/// A channel that can be used to send and receive messages.
pub trait Duplex<T>:
    futures::Stream<Item = Result<T, io::Error>>
    + futures::Sink<T, Error = io::Error>
    + Send
    + Sync
    + Unpin
{
    /// Creates a future that attempts to resolve the next item in the stream.
    /// If an error is encountered before the next item, the error is returned
    /// instead.
    ///
    /// Additionally, if the stream ends before the next item, an error is
    /// returned.
    ///
    /// This is similar to the [`TryStreamExt::try_next`](futures::stream::TryStreamExt::try_next)
    /// combinator, but returns an error if the stream ends before the next item instead of an
    /// `Option`.
    ///
    /// # Examples
    ///
    /// ```
    /// # futures::executor::block_on(async {
    /// use futures::{SinkExt, StreamExt};
    /// use utils_aio::duplex::{Duplex, MpscDuplex};
    ///
    /// let (mut a, mut b) = MpscDuplex::new();
    ///
    /// a.send(()).await.unwrap();
    /// a.close().await.unwrap();
    ///
    /// assert!(b.expect_next().await.is_ok());
    /// assert!(b.expect_next().await.is_err());
    /// # })
    /// ```
    fn expect_next(&mut self) -> ExpectNext<'_, Self>
    where
        Self: Sized,
    {
        ExpectNext::new(self)
    }
}

impl<T, U> Duplex<T> for U where
    U: futures::Stream<Item = Result<T, io::Error>>
        + futures::Sink<T, Error = io::Error>
        + Send
        + Sync
        + Unpin
{
}

#[derive(Debug)]
pub struct MpscDuplex<T> {
    sink: mpsc::Sender<T>,
    stream: mpsc::Receiver<T>,
}

impl<T> MpscDuplex<T>
where
    T: Send + 'static,
{
    pub fn new() -> (Self, Self) {
        let (sender, receiver) = mpsc::channel(10);
        let (sender_2, receiver_2) = mpsc::channel(10);
        (
            Self {
                sink: sender,
                stream: receiver_2,
            },
            Self {
                sink: sender_2,
                stream: receiver,
            },
        )
    }
}

impl<T> Sink<T> for MpscDuplex<T>
where
    T: Send + 'static,
{
    type Error = std::io::Error;

    fn poll_ready(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.sink)
            .poll_ready(cx)
            .map_err(|e| Error::new(ErrorKind::ConnectionAborted, e.to_string()))
    }

    fn start_send(mut self: std::pin::Pin<&mut Self>, item: T) -> Result<(), Self::Error> {
        Pin::new(&mut self.sink)
            .start_send(item)
            .map_err(|e| Error::new(ErrorKind::ConnectionAborted, e.to_string()))
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.sink)
            .poll_flush(cx)
            .map_err(|e| Error::new(ErrorKind::ConnectionAborted, e.to_string()))
    }

    fn poll_close(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.sink)
            .poll_close(cx)
            .map_err(|e| Error::new(ErrorKind::ConnectionAborted, e.to_string()))
    }
}

impl<T> Stream for MpscDuplex<T> {
    type Item = Result<T, std::io::Error>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        Pin::new(&mut self.stream).poll_next(cx).map(|x| x.map(Ok))
    }
}
