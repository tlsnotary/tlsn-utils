pub mod adaptive_barrier;
#[cfg(feature = "codec")]
pub mod codec;
#[cfg(feature = "duplex")]
pub mod duplex;
pub mod executor;
pub mod expect_msg;
pub mod factory;
#[cfg(feature = "mux")]
pub mod mux;
pub mod non_blocking_backend;
pub mod sink;
pub mod stream;

/// Trait implemented by a type which can provide a duplex channel.
#[cfg(feature = "duplex")]
pub trait IoProvider<T> {
    /// The type of the duplex channel.
    type Duplex: duplex::Duplex<T>;

    /// Provides a duplex channel.
    fn provide_io(&mut self) -> &mut Self::Duplex;
}
