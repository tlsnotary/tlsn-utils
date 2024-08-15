//! Memory channels for sending and receiving serializable types. Useful for testing.

use std::{
    any::Any,
    io::{Error, ErrorKind},
    pin::Pin,
    task::{Context, Poll},
};

use futures_channel::mpsc;
use futures_core::Stream as _;
use futures_sink::Sink as _;

use crate::{Deserialize, Serialize, Sink, Stream};

type Item = Box<dyn Any + Send + Sync + 'static>;

/// A memory sink that can be used to send any serializable type to the receiver.
#[derive(Debug, Clone)]
pub struct MemorySink(mpsc::Sender<Item>);

impl Sink for MemorySink {
    type Error = Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.0)
            .poll_ready(cx)
            .map_err(|e| Error::new(ErrorKind::ConnectionAborted, e))
    }

    fn start_send<Item: Serialize>(
        mut self: Pin<&mut Self>,
        item: Item,
    ) -> Result<(), Self::Error> {
        Pin::new(&mut self.0)
            .start_send(Box::new(item))
            .map_err(|e| Error::new(ErrorKind::ConnectionAborted, e))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.0)
            .poll_flush(cx)
            .map_err(|e| Error::new(ErrorKind::ConnectionAborted, e))
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.0)
            .poll_close(cx)
            .map_err(|e| Error::new(ErrorKind::ConnectionAborted, e))
    }
}

/// A memory stream that can be used to receive any deserializable type from the sender.
#[derive(Debug)]
pub struct MemoryStream(mpsc::Receiver<Item>);

impl Stream for MemoryStream {
    type Error = Error;

    fn poll_next<Item: Deserialize>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Item, Self::Error>>> {
        Pin::new(&mut self.0).poll_next(cx).map(|item| {
            item.map(|item| {
                item.downcast().map(|item| *item).map_err(|_| {
                    Error::new(ErrorKind::InvalidData, "sender sent an unexpected type")
                })
            })
        })
    }
}

/// Creates a new memory channel with the specified buffer size.
pub fn channel(buffer: usize) -> (MemorySink, MemoryStream) {
    let (sender, receiver) = mpsc::channel(buffer);
    (MemorySink(sender), MemoryStream(receiver))
}

/// An unbounded memory sink that can be used to send any serializable type to the receiver.
#[derive(Debug, Clone)]
pub struct UnboundedMemorySink(mpsc::UnboundedSender<Item>);

impl Sink for UnboundedMemorySink {
    type Error = Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.0)
            .poll_ready(cx)
            .map_err(|e| Error::new(ErrorKind::ConnectionAborted, e))
    }

    fn start_send<Item: Serialize>(
        mut self: Pin<&mut Self>,
        item: Item,
    ) -> Result<(), Self::Error> {
        Pin::new(&mut self.0)
            .start_send(Box::new(item))
            .map_err(|e| Error::new(ErrorKind::ConnectionAborted, e))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.0)
            .poll_flush(cx)
            .map_err(|e| Error::new(ErrorKind::ConnectionAborted, e))
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.0)
            .poll_close(cx)
            .map_err(|e| Error::new(ErrorKind::ConnectionAborted, e))
    }
}

/// An Unbounded memory stream that can be used to receive any deserializable type from the sender.
#[derive(Debug)]
pub struct UnboundedMemoryStream(mpsc::UnboundedReceiver<Item>);

impl Stream for UnboundedMemoryStream {
    type Error = Error;

    fn poll_next<Item: Deserialize>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Item, Self::Error>>> {
        Pin::new(&mut self.0).poll_next(cx).map(|item| {
            item.map(|item| {
                item.downcast().map(|item| *item).map_err(|_| {
                    Error::new(ErrorKind::InvalidData, "sender sent an unexpected type")
                })
            })
        })
    }
}

/// Creates a new memory channel with the specified buffer size.
pub fn unbounded() -> (UnboundedMemorySink, UnboundedMemoryStream) {
    let (sender, receiver) = mpsc::unbounded();
    (UnboundedMemorySink(sender), UnboundedMemoryStream(receiver))
}

/// A memory duplex that can be used to send and receive any serializable types.
#[derive(Debug)]
pub struct MemoryDuplex {
    sink: MemorySink,
    stream: MemoryStream,
}

impl MemoryDuplex {
    /// Returns the inner sink and stream.
    pub fn into_inner(self) -> (MemorySink, MemoryStream) {
        (self.sink, self.stream)
    }

    /// Returns a reference to the inner sink.
    pub fn sink_mut(&mut self) -> &mut MemorySink {
        &mut self.sink
    }

    /// Returns a reference to the inner stream.
    pub fn stream_mut(&mut self) -> &mut MemoryStream {
        &mut self.stream
    }
}

impl Sink for MemoryDuplex {
    type Error = Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.sink).poll_ready(cx)
    }

    fn start_send<Item: Serialize>(
        mut self: Pin<&mut Self>,
        item: Item,
    ) -> Result<(), Self::Error> {
        Pin::new(&mut self.sink).start_send(item)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.sink).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.sink).poll_close(cx)
    }
}

impl Stream for MemoryDuplex {
    type Error = Error;

    fn poll_next<Item: Deserialize>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Item, Self::Error>>> {
        Pin::new(&mut self.stream).poll_next(cx)
    }
}

/// Creates a new memory duplex with the specified buffer size.
pub fn duplex(buffer: usize) -> (MemoryDuplex, MemoryDuplex) {
    let (a, b) = channel(buffer);
    let (c, d) = channel(buffer);
    (
        MemoryDuplex { sink: a, stream: d },
        MemoryDuplex { sink: c, stream: b },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::{SinkExt, StreamExt};

    #[test]
    fn test_channel() {
        let (mut sink, mut stream) = channel(1);

        futures::executor::block_on(async {
            sink.send(42u8).await.unwrap();
            assert_eq!(stream.next::<u8>().await.unwrap().unwrap(), 42);
        })
    }

    #[test]
    #[should_panic]
    fn test_channel_type_mismatch() {
        let (mut sink, mut stream) = channel(1);

        futures::executor::block_on(async {
            sink.send(42u16).await.unwrap();
            stream.next::<u8>().await.unwrap().unwrap();
        })
    }

    #[test]
    fn test_duplex() {
        let (mut a, mut b) = duplex(1);

        futures::executor::block_on(async {
            a.send(42u8).await.unwrap();
            assert_eq!(b.next::<u8>().await.unwrap().unwrap(), 42);
        })
    }

    #[test]
    #[should_panic]
    fn test_duplex_type_mismatch() {
        let (mut a, mut b) = duplex(1);

        futures::executor::block_on(async {
            a.send(42u16).await.unwrap();
            b.next::<u8>().await.unwrap().unwrap();
        })
    }
}
