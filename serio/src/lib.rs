#![doc = include_str!("../README.md")]
#![deny(missing_docs, unreachable_pub, unused_must_use)]
#![deny(clippy::all)]

#[cfg(feature = "channel")]
pub mod channel;
#[cfg(feature = "codec")]
pub mod codec;
pub(crate) mod future;
pub mod sink;
pub mod stream;

#[cfg(feature = "codec")]
pub use codec::{Deserializer, Framed, Serializer};
pub use sink::{IoSink, Sink, SinkExt};
pub use stream::{IoStream, Stream, StreamExt};

/// A serializable type.
pub trait Serialize: serde::Serialize + Send + Sync + Unpin + 'static {}

impl<T> Serialize for T where T: serde::Serialize + Send + Sync + Unpin + 'static {}

/// A deserializable type.
pub trait Deserialize: serde::de::DeserializeOwned + Send + Sync + Unpin + 'static {}

impl<T> Deserialize for T where T: serde::de::DeserializeOwned + Send + Sync + Unpin + 'static {}

/// A duplex.
pub trait Duplex<T, E>: Sink<Error = E> + Stream<Error = E> {}

impl<T, U, E> Duplex<U, E> for T where T: Sink<Error = E> + Stream<Error = E> {}

/// A duplex with a `std::io::Error` error type.
pub trait IoDuplex<T>: Duplex<T, std::io::Error> {}

impl<T, U> IoDuplex<U> for T where T: Duplex<U, std::io::Error> {}
