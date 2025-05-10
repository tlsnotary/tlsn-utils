//! Utilities for converting framed transports to streams and sinks using a
//! codec.

use std::{
    io::{Error, ErrorKind},
    pin::Pin,
    task::{Context, Poll, ready},
};

use bytes::{Bytes, BytesMut};
use futures_core::stream::TryStream;
use futures_io::{AsyncRead, AsyncWrite};
use pin_project_lite::pin_project;
use tokio_util::codec::{Framed as TokioFramed, LengthDelimitedCodec};

use crate::{Deserialize, IoDuplex, Serialize, Sink, Stream};

/// A codec.
pub trait Codec<Io> {
    /// The framed transport type.
    type Framed: IoDuplex;

    /// Creates a new framed transport with the given IO.
    fn new_framed(&self, io: Io) -> Self::Framed;
}

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
            deserialize(buf)
        }
    }

    use tokio_util::{
        codec::{Framed as TokioFramed, LengthDelimitedCodec},
        compat::{Compat, FuturesAsyncReadCompatExt as _},
    };

    impl<Io> Codec<Io> for Bincode
    where
        Io: AsyncRead + AsyncWrite + Unpin,
    {
        type Framed = Framed<TokioFramed<Compat<Io>, LengthDelimitedCodec>, Self>;

        fn new_framed(&self, io: Io) -> Self::Framed {
            Framed::new(
                LengthDelimitedCodec::builder().new_framed(io.compat()),
                self.clone(),
            )
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

impl<T, U> Framed<TokioFramed<T, LengthDelimitedCodec>, U> {
    /// Sets a new maximum frame length and returns a [`WithFrameLimit`].
    ///
    /// # Arguments
    ///
    /// * `max_frame_len` - The new maximum frame length in bytes.
    pub fn with_max_frame_limit(&mut self, max_frame_len: usize) -> WithFrameLimit<'_, T, U> {
        let old_frame_limit = self.inner.codec().max_frame_length();
        self.inner.codec_mut().set_max_frame_length(max_frame_len);

        WithFrameLimit {
            old_frame_limit,
            framed: self,
        }
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

pin_project! {
    /// Wrapper around [`Framed`] to temporarily set a new maximum frame length.
    pub struct WithFrameLimit<'a, T, U> {
        old_frame_limit: usize,
        #[pin]
        framed: &'a mut Framed<TokioFramed<T, LengthDelimitedCodec>, U>,
    }

    impl<'a, T, U> PinnedDrop for WithFrameLimit<'a, T, U> {
        fn drop(this: Pin<&mut Self>) {
            let frame_limit = this.old_frame_limit;
            this.project().framed
                .inner
                .codec_mut()
                .set_max_frame_length(frame_limit);
        }
    }
}

impl<T, U> Stream for WithFrameLimit<'_, T, U>
where
    TokioFramed<T, LengthDelimitedCodec>: TryStream<Ok = BytesMut, Error = Error> + Unpin,
    U: Deserializer + Unpin,
    <U as Deserializer>::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type Error = Error;

    fn poll_next<Item: Deserialize>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Item, Error>>> {
        self.project().framed.poll_next(cx)
    }
}

impl<T, U> Sink for WithFrameLimit<'_, T, U>
where
    TokioFramed<T, LengthDelimitedCodec>: futures_sink::Sink<Bytes, Error = Error> + Unpin,
    U: Serializer + Unpin,
    <U as Serializer>::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type Error = Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().framed.poll_ready(cx)
    }

    fn start_send<I: Serialize>(
        self: std::pin::Pin<&mut Self>,
        item: I,
    ) -> Result<(), Self::Error> {
        self.project().framed.start_send(item)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().framed.poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().framed.poll_close(cx)
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};
    use tokio::io::duplex;
    use tokio_util::compat::TokioAsyncReadCompatExt;

    use crate::{SinkExt, StreamExt};

    use super::*;

    #[derive(Serialize, Deserialize)]
    struct Ping;

    #[derive(Serialize, Deserialize)]
    struct Pong;

    #[test]
    fn test_framed() {
        let (a, b) = duplex(1024);

        let mut a = Bincode.new_framed(a.compat());
        let mut b = Bincode.new_framed(b.compat());

        let a = async {
            a.send(Ping).await.unwrap();
            a.next::<Pong>().await.unwrap().unwrap();
        };

        let b = async {
            b.next::<Ping>().await.unwrap().unwrap();
            b.send(Pong).await.unwrap();
        };

        futures::executor::block_on(async {
            futures::join!(a, b);
        });
    }

    #[test]
    fn test_with_frame_limit() {
        let (a, b) = duplex(1024);

        let mut a = Bincode.new_framed(a.compat());
        let mut b = Bincode.new_framed(b.compat());

        let old_limit = a.inner.codec().max_frame_length();
        let new_limit = 2 * old_limit;

        {
            a.with_max_frame_limit(new_limit);
            b.with_max_frame_limit(new_limit);
        }

        assert_eq!(a.inner.codec().max_frame_length(), old_limit);
        assert_eq!(b.inner.codec().max_frame_length(), old_limit);
    }
}
