use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

use futures::{future::FusedFuture, stream::FusedStream, Future, Stream, TryStream, TryStreamExt};

/// A stream which yields `std::io::Result<T>` items.
pub trait IoStream<T>: Stream<Item = Result<T, std::io::Error>> {}

impl<T, U> IoStream<U> for T where T: Stream<Item = Result<U, std::io::Error>> {}

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

/// Extension trait for [`TryStream`](futures::stream::TryStream).
pub trait ExpectStreamExt: TryStream + Unpin {
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
    /// use utils_aio::stream::ExpectStreamExt;
    /// use utils_aio::duplex::MemoryDuplex;
    ///
    /// let (mut a, mut b) = MemoryDuplex::new();
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

impl<T> ExpectStreamExt for T where T: TryStream + Unpin {}
