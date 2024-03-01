//! Utilities for converting framed transports to streams and sinks using a codec.

use std::{
    io::{Error, ErrorKind},
    pin::Pin,
    task::{ready, Context, Poll},
};

use bytes::{Bytes, BytesMut};
use futures_core::stream::TryStream;

use crate::{Deserialize, Serialize, Sink, Stream};

/// A serializer.
pub trait Serializer {
    /// The error type.
    type Error;

    /// Serializes `item` into a buffer.
    fn serialize<T: Serialize>(&mut self, item: &T) -> Result<Bytes, Self::Error>;
}

/// A deserializer.
pub trait Deserializer {
    /// The error type.
    type Error;

    /// Deserializes a buffer into a value.
    fn deserialize<T: Deserialize>(&mut self, buf: &BytesMut) -> Result<T, Self::Error>;
}

#[cfg(feature = "bincode")]
mod bincode_impl {
    use super::*;
    use bincode::{deserialize, serialize};

    /// A bincode codec.
    #[derive(Default, Clone)]
    pub struct Bincode;

    impl Serializer for Bincode {
        type Error = bincode::Error;

        fn serialize<T: Serialize>(&mut self, item: &T) -> Result<Bytes, Self::Error> {
            Ok(Bytes::from(serialize(item)?))
        }
    }

    impl Deserializer for Bincode {
        type Error = bincode::Error;

        fn deserialize<T: Deserialize>(&mut self, buf: &BytesMut) -> Result<T, Self::Error> {
            Ok(deserialize(buf)?)
        }
    }
}

#[cfg(feature = "bincode")]
pub use bincode_impl::Bincode;

/// A framed transport.
pub struct Framed<T, C> {
    inner: T,
    codec: C,
}

impl<T, C> Framed<T, C> {
    /// Creates a new `Framed` with the given transport and codec.
    pub fn new(inner: T, codec: C) -> Self {
        Self { inner, codec }
    }
}

impl<T, C> Sink for Framed<T, C>
where
    T: futures_sink::Sink<Bytes, Error = Error> + Unpin,
    C: Serializer + Unpin,
    <C as Serializer>::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type Error = Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.inner).poll_ready(cx)
    }

    fn start_send<I: Serialize>(
        mut self: std::pin::Pin<&mut Self>,
        item: I,
    ) -> Result<(), Self::Error> {
        let buf = self
            .codec
            .serialize(&item)
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?;

        Pin::new(&mut self.inner).start_send(buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.inner).poll_close(cx)
    }
}

impl<T, C> Stream for Framed<T, C>
where
    T: TryStream<Ok = BytesMut, Error = Error> + Unpin,
    C: Deserializer + Unpin,
    <C as Deserializer>::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type Error = Error;

    fn poll_next<Item: Deserialize>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Item, Error>>> {
        let Some(buf) = ready!(Pin::new(&mut self.inner).try_poll_next(cx)) else {
            return Poll::Ready(None);
        };

        let item = self
            .codec
            .deserialize(&buf?)
            .map_err(|e| Error::new(ErrorKind::InvalidData, e));

        Poll::Ready(Some(item))
    }
}
