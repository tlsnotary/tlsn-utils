//! Stream types and traits.

use std::{
    future::Future,
    marker::PhantomData,
    ops::DerefMut,
    pin::Pin,
    task::{ready, Context, Poll},
};

use futures_core::FusedFuture;

use crate::{future::assert_future, Deserialize};

/// A stream with an error type of `std::io::Error`.
pub trait IoStream: Stream<Error = std::io::Error> {}

impl<T: ?Sized> IoStream for T where T: Stream<Error = std::io::Error> {}

/// A stream producing any kind of value which implements `Deserialize`.
///
/// This trait is similar to [`futures::Stream`](https://docs.rs/futures/latest/futures/stream/trait.Stream.html),
/// but facilitates receiving of any deserializable type instead of a single type.
#[must_use = "streams do nothing unless polled"]
pub trait Stream {
    /// The type of value produced by the stream when an error occurs.
    type Error;

    /// Attempt to pull out the next value of this stream, registering the
    /// current task for wakeup if the value is not yet available, and returning
    /// `None` if the stream is exhausted.
    ///
    /// # Return value
    ///
    /// There are several possible return values, each indicating a distinct
    /// stream state:
    ///
    /// - `Poll::Pending` means that this stream's next value is not ready
    /// yet. Implementations will ensure that the current task will be notified
    /// when the next value may be ready.
    ///
    /// - `Poll::Ready(Some(val))` means that the stream has successfully
    /// produced a value, `val`, and may produce further values on subsequent
    /// `poll_next` calls.
    ///
    /// - `Poll::Ready(None)` means that the stream has terminated, and
    /// `poll_next` should not be invoked again.
    fn poll_next<Item: Deserialize>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Item, Self::Error>>>;

    /// Returns the bounds on the remaining length of the stream.
    ///
    /// Specifically, `size_hint()` returns a tuple where the first element
    /// is the lower bound, and the second element is the upper bound.
    ///
    /// The second half of the tuple that is returned is an [`Option`]`<`[`usize`]`>`.
    /// A [`None`] here means that either there is no known upper bound, or the
    /// upper bound is larger than [`usize`].
    ///
    /// # Implementation notes
    ///
    /// It is not enforced that a stream implementation yields the declared
    /// number of elements. A buggy stream may yield less than the lower bound
    /// or more than the upper bound of elements.
    ///
    /// `size_hint()` is primarily intended to be used for optimizations such as
    /// reserving space for the elements of the stream, but must not be
    /// trusted to e.g., omit bounds checks in unsafe code. An incorrect
    /// implementation of `size_hint()` should not lead to memory safety
    /// violations.
    ///
    /// That said, the implementation should provide a correct estimation,
    /// because otherwise it would be a violation of the trait's protocol.
    ///
    /// The default implementation returns `(0, `[`None`]`)` which is correct for any
    /// stream.
    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, None)
    }
}

impl<S: ?Sized + Stream + Unpin> Stream for &mut S {
    type Error = S::Error;

    fn poll_next<Item: Deserialize>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Item, Self::Error>>> {
        S::poll_next(Pin::new(&mut **self), cx)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (**self).size_hint()
    }
}

impl<P> Stream for Pin<P>
where
    P: DerefMut + Unpin,
    P::Target: Stream,
{
    type Error = <P::Target as Stream>::Error;

    fn poll_next<Item: Deserialize>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Item, Self::Error>>> {
        self.get_mut().as_mut().poll_next(cx)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (**self).size_hint()
    }
}

/// A stream which tracks whether or not the underlying stream
/// should no longer be polled.
///
/// `is_terminated` will return `true` if a future should no longer be polled.
/// Usually, this state occurs after `poll_next` (or `try_poll_next`) returned
/// `Poll::Ready(None)`. However, `is_terminated` may also return `true` if a
/// stream has become inactive and can no longer make progress and should be
/// ignored or dropped rather than being polled again.
pub trait FusedStream: Stream {
    /// Returns `true` if the stream should no longer be polled.
    fn is_terminated(&self) -> bool;
}

impl<F: ?Sized + FusedStream + Unpin> FusedStream for &mut F {
    fn is_terminated(&self) -> bool {
        <F as FusedStream>::is_terminated(&**self)
    }
}

impl<P> FusedStream for Pin<P>
where
    P: DerefMut + Unpin,
    P::Target: FusedStream,
{
    fn is_terminated(&self) -> bool {
        <P::Target as FusedStream>::is_terminated(&**self)
    }
}

/// An extension trait for Streams that provides a variety of convenient functions.
pub trait StreamExt: Stream {
    /// Creates a future that resolves to the next item in the stream.
    ///
    /// Note that because `next` doesn't take ownership over the stream,
    /// the [`Stream`] type must be [`Unpin`]. If you want to use `next` with a
    /// [`!Unpin`](Unpin) stream, you'll first have to pin the stream. This can
    /// be done by boxing the stream using [`Box::pin`] or
    /// pinning it to the stack using the `pin_mut!` macro from the `pin_utils`
    /// crate.
    ///
    /// # Examples
    ///
    /// ```
    /// # futures::executor::block_on(async {
    /// use futures::stream::{self, StreamExt};
    ///
    /// let mut stream = stream::iter(1..=3);
    ///
    /// assert_eq!(stream.next().await, Some(1));
    /// assert_eq!(stream.next().await, Some(2));
    /// assert_eq!(stream.next().await, Some(3));
    /// assert_eq!(stream.next().await, None);
    /// # });
    /// ```
    fn next<Item: Deserialize>(&mut self) -> Next<'_, Self, Item>
    where
        Self: Unpin,
    {
        assert_future::<Option<Result<Item, Self::Error>>, _>(Next::new(self))
    }

    /// A convenience method for calling [`Stream::poll_next`] on [`Unpin`]
    /// stream types.
    fn poll_next_unpin<Item: Deserialize>(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Item, Self::Error>>>
    where
        Self: Unpin,
    {
        Pin::new(self).poll_next(cx)
    }
}

impl<S: Stream + ?Sized> StreamExt for S {}

/// Future for the [`next`](StreamExt::next) method.
#[derive(Debug)]
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct Next<'a, St: ?Sized, Item> {
    stream: &'a mut St,
    _pd: PhantomData<Item>,
}

impl<St: ?Sized + Unpin, Item> Unpin for Next<'_, St, Item> {}

impl<'a, St: ?Sized + Stream + Unpin, Item> Next<'a, St, Item> {
    pub(super) fn new(stream: &'a mut St) -> Self {
        Self {
            stream,
            _pd: PhantomData,
        }
    }
}

impl<St: ?Sized + FusedStream + Unpin, Item: Deserialize> FusedFuture for Next<'_, St, Item> {
    fn is_terminated(&self) -> bool {
        self.stream.is_terminated()
    }
}

impl<St: ?Sized + Stream + Unpin, Item: Deserialize> Future for Next<'_, St, Item> {
    type Output = Option<Result<Item, St::Error>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.stream.poll_next_unpin(cx)
    }
}

/// An extension trait for [`IoStream`] which provides a variety of convenient functions.
pub trait IoStreamExt: IoStream {
    /// Creates a future that resolves to the next item in the stream, returning
    /// an error if the stream is exhausted.
    fn expect_next<Item: Deserialize>(&mut self) -> ExpectNext<'_, Self, Item>
    where
        Self: Unpin,
    {
        ExpectNext {
            next: self.next(),
            _pd: PhantomData,
        }
    }
}

impl<S: ?Sized> IoStreamExt for S where S: IoStream {}

/// Future for the [`expect_next`](IoStreamExt::expect_next) method.
#[derive(Debug)]
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct ExpectNext<'a, St: ?Sized, Item> {
    next: Next<'a, St, Item>,
    _pd: PhantomData<Item>,
}

impl<St: ?Sized + Unpin, Item> Unpin for ExpectNext<'_, St, Item> {}

impl<'a, St: ?Sized + IoStream + Unpin, Item: Deserialize> Future for ExpectNext<'a, St, Item> {
    type Output = Result<Item, St::Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match ready!(Pin::new(&mut self.next).poll(cx)) {
            Some(Ok(item)) => Poll::Ready(Ok(item)),
            Some(Err(err)) => Poll::Ready(Err(err)),
            None => Poll::Ready(Err(std::io::ErrorKind::UnexpectedEof.into())),
        }
    }
}
