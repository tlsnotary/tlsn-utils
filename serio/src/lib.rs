#![doc = include_str!("../README.md")]
#![deny(missing_docs, unreachable_pub, unused_must_use)]
#![deny(clippy::all)]

#[cfg(feature = "channel")]
pub mod channel;
#[cfg(feature = "codec")]
pub mod codec;
#[cfg(feature = "compat")]
pub(crate) mod compat;
pub(crate) mod future;
pub mod sink;
pub mod stream;

#[cfg(feature = "codec")]
pub use codec::{Deserializer, Framed, Serializer};
#[cfg(feature = "compat")]
pub use compat::FuturesCompat;
pub use sink::{IoSink, Sink, SinkExt};
pub use stream::{IoStream, Stream, StreamExt};

/// A serializable type.
pub trait Serialize: serde::Serialize + Send + Sync + Unpin + 'static {}

impl<T> Serialize for T where T: serde::Serialize + Send + Sync + Unpin + 'static {}

/// A deserializable type.
pub trait Deserialize: serde::de::DeserializeOwned + Send + Sync + Unpin + 'static {}

impl<T> Deserialize for T where T: serde::de::DeserializeOwned + Send + Sync + Unpin + 'static {}

/// A duplex.
pub trait Duplex: Sink + Stream {}

impl<T: ?Sized> Duplex for T where T: Sink + Stream {}

/// A duplex with a `std::io::Error` error type.
pub trait IoDuplex: Sink<Error = std::io::Error> + Stream<Error = std::io::Error> {}

impl<T: ?Sized> IoDuplex for T where T: Sink<Error = std::io::Error> + Stream<Error = std::io::Error>
{}
