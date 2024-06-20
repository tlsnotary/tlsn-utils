use std::{
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll},
};

use crate::{Deserialize, Stream};

use super::*;

pin_project_lite::pin_project! {
    /// Compatibility wrapper for futures traits.
    pub struct FuturesCompat<T, Item> {
        #[pin]
        inner: T,
        _pd: PhantomData<Item>,
    }
}

impl<T, Item> FuturesCompat<T, Item> {
    /// Creates a new `FuturesCompat`.
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            _pd: PhantomData,
        }
    }

    /// Returns a reference to the inner object.
    pub fn inner(&self) -> &T {
        &self.inner
    }

    /// Returns a mutable reference to the inner object.
    pub fn inner_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    /// Returns the inner object.
    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T, Item> futures_sink::Sink<Item> for FuturesCompat<T, Item>
where
    T: Sink,
    Item: Serialize,
{
    type Error = T::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().inner.poll_ready(cx)
    }

    fn start_send(self: Pin<&mut Self>, item: Item) -> Result<(), Self::Error> {
        self.project().inner.start_send(item)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().inner.poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().inner.poll_close(cx)
    }
}

impl<T, Item> futures_core::Stream for FuturesCompat<T, Item>
where
    T: Stream,
    Item: Deserialize,
{
    type Item = Result<Item, T::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.project().inner.poll_next(cx)
    }
}
