//! Sink types and traits.

use std::{
    future::Future,
    marker::PhantomData,
    ops::DerefMut,
    pin::Pin,
    task::{ready, Context, Poll},
};

use crate::{future::assert_future, Serialize};

/// A sink which accepts any item which implements `Serialize`.
///
/// This trait is similar to [`futures::Sink`](https://docs.rs/futures/latest/futures/sink/trait.Sink.html),
/// but facilitates sending of any serializable type instead of a single type.
#[must_use = "sinks do nothing unless polled"]
pub trait Sink {
    /// The type of value produced by the sink when an error occurs.
    type Error;

    /// Attempts to prepare the `Sink` to receive a value.
    ///
    /// This method must be called and return `Poll::Ready(Ok(()))` prior to
    /// each call to `start_send`.
    ///
    /// This method returns `Poll::Ready` once the underlying sink is ready to
    /// receive data. If this method returns `Poll::Pending`, the current task
    /// is registered to be notified (via `cx.waker().wake_by_ref()`) when `poll_ready`
    /// should be called again.
    ///
    /// In most cases, if the sink encounters an error, the sink will
    /// permanently be unable to receive items.
    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>>;

    /// Begin the process of sending a value to the sink.
    /// Each call to this function must be preceded by a successful call to
    /// `poll_ready` which returned `Poll::Ready(Ok(()))`.
    ///
    /// As the name suggests, this method only *begins* the process of sending
    /// the item. If the sink employs buffering, the item isn't fully processed
    /// until the buffer is fully flushed. Since sinks are designed to work with
    /// asynchronous I/O, the process of actually writing out the data to an
    /// underlying object takes place asynchronously. **You *must* use
    /// `poll_flush` or `poll_close` in order to guarantee completion of a
    /// send**.
    ///
    /// Implementations of `poll_ready` and `start_send` will usually involve
    /// flushing behind the scenes in order to make room for new messages.
    /// It is only necessary to call `poll_flush` if you need to guarantee that
    /// *all* of the items placed into the `Sink` have been sent.
    ///
    /// In most cases, if the sink encounters an error, the sink will
    /// permanently be unable to receive items.
    fn start_send<Item: Serialize>(self: Pin<&mut Self>, item: Item) -> Result<(), Self::Error>;

    /// Flush any remaining output from this sink.
    ///
    /// Returns `Poll::Ready(Ok(()))` when no buffered items remain. If this
    /// value is returned then it is guaranteed that all previous values sent
    /// via `start_send` have been flushed.
    ///
    /// Returns `Poll::Pending` if there is more work left to do, in which
    /// case the current task is scheduled (via `cx.waker().wake_by_ref()`) to wake up when
    /// `poll_flush` should be called again.
    ///
    /// In most cases, if the sink encounters an error, the sink will
    /// permanently be unable to receive items.
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>>;

    /// Flush any remaining output and close this sink, if necessary.
    ///
    /// Returns `Poll::Ready(Ok(()))` when no buffered items remain and the sink
    /// has been successfully closed.
    ///
    /// Returns `Poll::Pending` if there is more work left to do, in which
    /// case the current task is scheduled (via `cx.waker().wake_by_ref()`) to wake up when
    /// `poll_close` should be called again.
    ///
    /// If this function encounters an error, the sink should be considered to
    /// have failed permanently, and no more `Sink` methods should be called.
    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>>;
}

impl<S: ?Sized + Sink + Unpin> Sink for &mut S {
    type Error = S::Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut **self).poll_ready(cx)
    }

    fn start_send<Item: Serialize>(
        mut self: Pin<&mut Self>,
        item: Item,
    ) -> Result<(), Self::Error> {
        Pin::new(&mut **self).start_send(item)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut **self).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut **self).poll_close(cx)
    }
}

impl<P> Sink for Pin<P>
where
    P: DerefMut + Unpin,
    P::Target: Sink,
{
    type Error = <P::Target as Sink>::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.get_mut().as_mut().poll_ready(cx)
    }

    fn start_send<Item: Serialize>(self: Pin<&mut Self>, item: Item) -> Result<(), Self::Error> {
        self.get_mut().as_mut().start_send(item)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.get_mut().as_mut().poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.get_mut().as_mut().poll_close(cx)
    }
}

/// An extension trait for Sinks that provides a variety of convenient functions.
pub trait SinkExt: Sink {
    /// Close the sink.
    fn close(&mut self) -> Close<'_, Self>
    where
        Self: Unpin,
    {
        assert_future::<Result<(), Self::Error>, _>(Close::new(self))
    }

    /// A future that completes after the given item has been fully processed
    /// into the sink, including flushing.
    ///
    /// Note that, **because of the flushing requirement, it is usually better
    /// to batch together items to send via `feed` or `send_all`,
    /// rather than flushing between each item.**
    fn send<Item: Serialize>(&mut self, item: Item) -> Send<'_, Self, Item>
    where
        Self: Unpin,
    {
        assert_future::<Result<(), Self::Error>, _>(Send::new(self, item))
    }

    /// A future that completes after the given item has been received
    /// by the sink.
    ///
    /// Unlike `send`, the returned future does not flush the sink.
    /// It is the caller's responsibility to ensure all pending items
    /// are processed, which can be done via `flush` or `close`.
    fn feed<Item: Serialize>(&mut self, item: Item) -> Feed<'_, Self, Item>
    where
        Self: Unpin,
    {
        assert_future::<Result<(), Self::Error>, _>(Feed::new(self, item))
    }

    /// Convert this sink into a `futures::Sink`.
    #[cfg(feature = "compat")]
    fn into_sink<Item: Serialize>(self) -> IntoSink<Self, Item>
    where
        Self: Sized,
    {
        IntoSink::new(self)
    }
}

impl<S: Sink + ?Sized> SinkExt for S {}

/// Future for the [`close`](super::SinkExt::close) method.
#[derive(Debug)]
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct Close<'a, Si: ?Sized> {
    sink: &'a mut Si,
}

impl<Si: Unpin + ?Sized> Unpin for Close<'_, Si> {}

/// A future that completes when the sink has finished closing.
///
/// The sink itself is returned after closing is complete.
impl<'a, Si: Sink + Unpin + ?Sized> Close<'a, Si> {
    fn new(sink: &'a mut Si) -> Self {
        Self { sink }
    }
}

impl<Si: Sink + Unpin + ?Sized> Future for Close<'_, Si> {
    type Output = Result<(), Si::Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.sink).poll_close(cx)
    }
}

/// Future for the [`send`](super::SinkExt::send) method.
#[derive(Debug)]
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct Send<'a, Si: ?Sized, Item> {
    feed: Feed<'a, Si, Item>,
}

// Pinning is never projected to children
impl<Si: Unpin + ?Sized, Item> Unpin for Send<'_, Si, Item> {}

impl<'a, Si: Sink + Unpin + ?Sized, Item> Send<'a, Si, Item> {
    fn new(sink: &'a mut Si, item: Item) -> Self {
        Self {
            feed: Feed::new(sink, item),
        }
    }
}

impl<Si: Sink + Unpin + ?Sized, Item: Serialize> Future for Send<'_, Si, Item> {
    type Output = Result<(), Si::Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = &mut *self;

        if this.feed.is_item_pending() {
            ready!(Pin::new(&mut this.feed).poll(cx))?;
            debug_assert!(!this.feed.is_item_pending());
        }

        // we're done sending the item, but want to block on flushing the
        // sink
        ready!(this.feed.sink_pin_mut().poll_flush(cx))?;

        Poll::Ready(Ok(()))
    }
}

/// Future for the [`feed`](super::SinkExt::feed) method.
#[derive(Debug)]
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct Feed<'a, Si: ?Sized, Item> {
    sink: &'a mut Si,
    item: Option<Item>,
}

// Pinning is never projected to children
impl<Si: Unpin + ?Sized, Item> Unpin for Feed<'_, Si, Item> {}

impl<'a, Si: Sink + Unpin + ?Sized, Item> Feed<'a, Si, Item> {
    fn new(sink: &'a mut Si, item: Item) -> Self {
        Feed {
            sink,
            item: Some(item),
        }
    }

    fn sink_pin_mut(&mut self) -> Pin<&mut Si> {
        Pin::new(self.sink)
    }

    fn is_item_pending(&self) -> bool {
        self.item.is_some()
    }
}

impl<Si: Sink + Unpin + ?Sized, Item: Serialize> Future for Feed<'_, Si, Item> {
    type Output = Result<(), Si::Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut sink = Pin::new(&mut this.sink);
        ready!(sink.as_mut().poll_ready(cx))?;
        let item = this.item.take().expect("polled Feed after completion");
        sink.as_mut().start_send(item)?;
        Poll::Ready(Ok(()))
    }
}

#[cfg(feature = "compat")]
mod compat {
    use super::*;

    /// Wraps a sink and provides a `futures::Sink` implementation.
    pub struct IntoSink<Si, Item>(Si, PhantomData<Item>);

    impl<Si, Item> IntoSink<Si, Item> {
        pub(super) fn new(sink: Si) -> Self {
            Self(sink, PhantomData)
        }

        /// Returns a reference to the inner sink.
        pub fn sink(&self) -> &Si {
            &self.0
        }

        /// Returns a mutable reference to the inner sink.
        pub fn sink_mut(&mut self) -> &mut Si {
            &mut self.0
        }

        /// Returns the inner sink.
        pub fn into_inner(self) -> Si {
            self.0
        }
    }

    impl<Si, Item> futures_sink::Sink<Item> for IntoSink<Si, Item>
    where
        Si: Sink<Error = std::io::Error> + Unpin,
        Item: Serialize,
    {
        type Error = std::io::Error;

        fn poll_ready(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Pin::new(&mut self.0).poll_ready(cx)
        }

        fn start_send(mut self: Pin<&mut Self>, item: Item) -> Result<(), Self::Error> {
            Pin::new(&mut self.0).start_send(item)
        }

        fn poll_flush(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Pin::new(&mut self.0).poll_flush(cx)
        }

        fn poll_close(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Pin::new(&mut self.0).poll_close(cx)
        }
    }
}

#[cfg(feature = "compat")]
pub use compat::IntoSink;
