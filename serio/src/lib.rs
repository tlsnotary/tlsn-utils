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
pub use sink::{Sink, SinkExt};
pub use stream::{Stream, StreamExt};

/// A sink with an error type of `std::io::Error`.
pub trait IoSink: Sink<Error = std::io::Error> {}

impl<T: ?Sized> IoSink for T where T: Sink<Error = std::io::Error> {}

/// A stream with an error type of `std::io::Error`.
pub trait IoStream: Stream<Error = std::io::Error> {}

impl<T: ?Sized> IoStream for T where T: Stream<Error = std::io::Error> {}

/// A serializable type.
pub trait Serialize: serde::Serialize + Send + Sync + Unpin + 'static {}

impl<T> Serialize for T where T: serde::Serialize + Send + Sync + Unpin + 'static {}

/// A deserializable type.
pub trait Deserialize: serde::de::DeserializeOwned + Send + Sync + Unpin + 'static {}

impl<T> Deserialize for T where T: serde::de::DeserializeOwned + Send + Sync + Unpin + 'static {}
