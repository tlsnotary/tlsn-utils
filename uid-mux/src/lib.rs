//! Multiplexing with unique channel ids.

#![deny(missing_docs, unreachable_pub, unused_must_use)]
#![deny(clippy::all)]
#![forbid(unsafe_code)]

pub(crate) mod future;
#[cfg(feature = "serio")]
mod serio;
#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;
pub mod yamux;

#[cfg(feature = "serio")]
pub use serio::{FramedMux, FramedUidMux};

use core::fmt;

use async_trait::async_trait;
use futures::io::{AsyncRead, AsyncWrite};

/// Internal stream identifier.
///
/// User provided ids are hashed to a fixed length.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct InternalId([u8; 32]);

impl InternalId {
    /// Create a new `InternalId` from a byte slice.
    pub(crate) fn new(bytes: &[u8]) -> Self {
        Self(blake3::hash(bytes).into())
    }
}

impl fmt::Display for InternalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0[..4] {
            write!(f, "{:02x}", byte)?;
        }
        Ok(())
    }
}

impl AsRef<[u8]> for InternalId {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// A multiplexer that opens streams with unique ids.
#[async_trait]
pub trait UidMux<Id> {
    /// Stream type.
    type Stream: AsyncWrite + AsyncRead + Send + Sync + Unpin + 'static;
    /// Error type.
    type Error;

    /// Open a new stream with the given id.
    async fn open(&self, id: &Id) -> Result<Self::Stream, Self::Error>;
}

pub(crate) mod log {
    macro_rules! error {
        ($( $tokens:tt )*) => {
            {
                #[cfg(feature = "tracing")]
                tracing::error!($( $tokens )*);
            }
        };
    }

    macro_rules! trace {
        ($( $tokens:tt )*) => {
            {
                #[cfg(feature = "tracing")]
                tracing::trace!($( $tokens )*);
            }
        };
    }

    macro_rules! debug {
        ($( $tokens:tt )*) => {
            {
                #[cfg(feature = "tracing")]
                tracing::debug!($( $tokens )*);
            }
        };
    }

    macro_rules! info {
        ($( $tokens:tt )*) => {
            {
                #[cfg(feature = "tracing")]
                tracing::info!($( $tokens )*);
            }
        };
    }

    pub(crate) use {debug, error, info, trace};
}
