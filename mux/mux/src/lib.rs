// Copyright (c) 2018-2019 Parity Technologies (UK) Ltd.
// Modifications Copyright (c) 2026 TLSNotary
//
// Licensed under the Apache License, Version 2.0 or MIT license, at your
// option.
//
// A copy of the Apache License, Version 2.0 is included in the software as
// LICENSE-APACHE and a copy of the MIT license is included in the software
// as LICENSE-MIT. You may also obtain a copy of the Apache License, Version 2.0
// at https://www.apache.org/licenses/LICENSE-2.0 and a copy of the MIT license
// at https://opensource.org/licenses/MIT.

//! A multiplexing library for TLSNotary.
//!
//! It multiplexes independent I/O streams over reliable, ordered connections,
//! such as TCP/IP.
//!
//! The two primary objects, clients of this crate interact with, are:
//!
//! - [`Connection`], which wraps the underlying I/O resource, e.g. a socket,
//!   and provides methods for opening outbound or accepting inbound streams.
//! - [`Stream`], which implements [`futures::io::AsyncRead`] and
//!   [`futures::io::AsyncWrite`].

#![forbid(unsafe_code)]

mod chunks;
mod error;
mod frame;

pub(crate) mod connection;
mod tagged_stream;
mod traffic;

pub use crate::{
    connection::{Connection, Handle, Stream},
    error::ConnectionError,
    frame::{
        FrameDecodeError,
        header::{HeaderDecodeError, StreamId},
    },
    traffic::Traffic,
};

const KIB: usize = 1024;
const MIB: usize = KIB * 1024;
const GIB: usize = MIB * 1024;

pub const DEFAULT_CREDIT: u32 = 256 * KIB as u32;

pub type Result<T> = std::result::Result<T, ConnectionError>;

/// Default maximum number of bytes a data frame might carry as its
/// payload when being send. Larger Payloads will be split.
///
/// This implementation restricts the size to:
///
/// 1. Reduce delays sending time-sensitive frames, e.g. window updates.
/// 2. Minimize head-of-line blocking across streams.
/// 3. Enable better interleaving of send and receive operations, as each is
///    carried out atomically instead of concurrently with its respective
///    counterpart.
const DEFAULT_SPLIT_SEND_SIZE: usize = 16 * KIB;

/// Multiplexer configuration.
///
/// The default configuration values are as follows:
///
/// - max. for the total receive window size across all streams of a connection
///   = 1 GiB
/// - max. number of streams = 512
/// - read after close = true
/// - split send size = 16 KiB
/// - close sync = false
/// - keep alive = false
#[derive(Debug, Clone)]
pub struct Config {
    max_connection_receive_window: Option<usize>,
    max_num_streams: usize,
    read_after_close: bool,
    split_send_size: usize,
    pub(crate) close_sync: bool,
    keep_alive: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            max_connection_receive_window: Some(GIB),
            max_num_streams: 512,
            read_after_close: true,
            split_send_size: DEFAULT_SPLIT_SEND_SIZE,
            close_sync: false,
            keep_alive: false,
        }
    }
}

impl Config {
    /// Creates a new [`ConfigBuilder`], initialized with the default
    /// configuration.
    ///
    /// The builder validates cross-field invariants at [`ConfigBuilder::build`]
    /// time and returns a [`ConfigError`] instead of panicking.
    pub fn builder() -> ConfigBuilder {
        ConfigBuilder::default()
    }
}

/// The error returned when a [`Config`] cannot be built.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// The configured `max_connection_receive_window` is smaller than
    /// `256 KiB * max_num_streams`, leaving some streams less than the default
    /// window.
    ReceiveWindowTooSmall {
        /// The configured total receive window, in bytes.
        max_connection_receive_window: usize,
        /// The configured maximum number of streams.
        max_num_streams: usize,
        /// The minimum total receive window required for `max_num_streams`, in
        /// bytes.
        min_required: usize,
    },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::ReceiveWindowTooSmall {
                max_connection_receive_window,
                max_num_streams,
                min_required,
            } => write!(
                f,
                "`max_connection_receive_window` ({max_connection_receive_window} bytes) is smaller \
                 than the {min_required} bytes required to give each of the {max_num_streams} \
                 streams the default window size"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

/// A builder for [`Config`].
#[derive(Debug, Clone, Default)]
pub struct ConfigBuilder {
    config: Config,
}

impl ConfigBuilder {
    /// Set the upper limit for the total receive window size across all streams
    /// of a connection.
    ///
    /// Must be `>= 256 KiB * max_num_streams` to allow each stream at least the
    /// default window size; otherwise [`build`](Self::build) fails.
    ///
    /// The window of a stream starts at 256 KiB and is increased (auto-tuned)
    /// based on the connection's round-trip time and the stream's bandwidth
    /// (striving for the bandwidth-delay-product).
    ///
    /// Set to `None` to disable limit, i.e. allow each stream to grow receive
    /// window based on connection's round-trip time and stream's bandwidth
    /// without limit.
    ///
    /// ## DOS attack mitigation
    ///
    /// A remote node (attacker) might trick the local node (target) into
    /// allocating large stream receive windows, trying to make the local
    /// node run out of memory.
    ///
    /// This attack is difficult, as the local node only increases the stream
    /// receive window up to 2x the bandwidth-delay-product, where bandwidth
    /// is the amount of bytes read, not just received. In other words, the
    /// attacker has to send (and have the local node read) significant
    /// amount of bytes on a stream over a long period of time to increase the
    /// stream receive window. E.g. on a 60ms 10Gbit/s connection the
    /// bandwidth-delay-product is ~75 MiB and thus the local node will at
    /// most allocate ~150 MiB (2x bandwidth-delay-product) per stream.
    ///
    /// Despite the difficulty of the attack one should choose a reasonable
    /// `max_connection_receive_window` to protect against this attack,
    /// especially since an attacker might use more than one stream per
    /// connection.
    pub fn max_connection_receive_window(mut self, n: Option<usize>) -> Self {
        self.config.max_connection_receive_window = n;
        self
    }

    /// Set the max. number of streams per connection.
    ///
    /// Clamped to at least 1: a stream becomes active on the wire only after
    /// claiming one of these slots, so a limit of 0 would make every write
    /// block forever.
    ///
    /// The limit is enforced as write backpressure: a write to a stream that
    /// is not yet active on the wire blocks until a slot frees. An application
    /// that holds more than this many streams active concurrently — and needs
    /// progress on all of them to make progress at all — will therefore
    /// deadlock; size the limit above the application's maximum number of
    /// mutually-dependent concurrent streams.
    ///
    /// Streams the peer opens implicitly — including ones the peer has already
    /// closed — hold a slot until a local handle claims them and is dropped;
    /// their data is never discarded. It is the application's responsibility
    /// to open streams deterministically on both sides so every peer-opened
    /// stream is eventually claimed.
    pub fn max_num_streams(mut self, n: usize) -> Self {
        self.config.max_num_streams = n.max(1);
        self
    }

    /// Allow or disallow streams to read from buffered data after
    /// the connection has been closed.
    pub fn read_after_close(mut self, b: bool) -> Self {
        self.config.read_after_close = b;
        self
    }

    /// Set the max. payload size used when sending data frames. Payloads larger
    /// than the configured max. will be split.
    pub fn split_send_size(mut self, n: usize) -> Self {
        self.config.split_send_size = n;
        self
    }

    /// Enable or disable synchronized close.
    ///
    /// When enabled, the initiating side will wait for a GoAway reply before
    /// completing the close. The receiving side will send a GoAway reply before
    /// closing.
    pub fn close_sync(mut self, b: bool) -> Self {
        self.config.close_sync = b;
        self
    }

    /// Enable or disable keep-alive pings.
    ///
    /// Note: This is currently a placeholder and has no effect.
    pub fn keep_alive(mut self, b: bool) -> Self {
        self.config.keep_alive = b;
        self
    }

    /// Builds the [`Config`], validating it.
    ///
    /// Returns [`ConfigError::ReceiveWindowTooSmall`] if the configured
    /// `max_connection_receive_window` cannot give every stream the default
    /// window size.
    pub fn build(self) -> std::result::Result<Config, ConfigError> {
        let max_num_streams = self.config.max_num_streams;
        let min_required = max_num_streams.saturating_mul(DEFAULT_CREDIT as usize);

        if let Some(window) = self.config.max_connection_receive_window
            && window < min_required
        {
            return Err(ConfigError::ReceiveWindowTooSmall {
                max_connection_receive_window: window,
                max_num_streams,
                min_required,
            });
        }

        Ok(self.config)
    }
}

// Check that we can safely cast a `usize` to a `u64`.
static_assertions::const_assert! {
    std::mem::size_of::<usize>() <= std::mem::size_of::<u64>()
}

// Check that we can safely cast a `u32` to a `usize`.
static_assertions::const_assert! {
    std::mem::size_of::<u32>() <= std::mem::size_of::<usize>()
}

#[cfg(test)]
impl quickcheck::Arbitrary for Config {
    fn arbitrary(g: &mut quickcheck::Gen) -> Self {
        use quickcheck::GenRange;

        let max_num_streams = g.gen_range(0..u16::MAX as usize);

        Config {
            max_connection_receive_window: if bool::arbitrary(g) {
                Some(g.gen_range((DEFAULT_CREDIT as usize * max_num_streams)..usize::MAX))
            } else {
                None
            },
            max_num_streams,
            read_after_close: bool::arbitrary(g),
            split_send_size: g.gen_range(DEFAULT_SPLIT_SEND_SIZE..usize::MAX),
            close_sync: bool::arbitrary(g),
            keep_alive: bool::arbitrary(g),
        }
    }
}

#[cfg(test)]
mod config_tests {
    use super::*;

    const CREDIT: usize = DEFAULT_CREDIT as usize;

    #[test]
    fn builder_default_equals_config_default() {
        let built = Config::builder().build().expect("default config is valid");
        let default = Config::default();

        assert_eq!(
            built.max_connection_receive_window,
            default.max_connection_receive_window
        );
        assert_eq!(built.max_num_streams, default.max_num_streams);
        assert_eq!(built.read_after_close, default.read_after_close);
        assert_eq!(built.split_send_size, default.split_send_size);
        assert_eq!(built.close_sync, default.close_sync);
        assert_eq!(built.keep_alive, default.keep_alive);
    }

    #[test]
    fn receive_window_too_small_is_an_error() {
        let err = Config::builder()
            .max_num_streams(512)
            .max_connection_receive_window(Some(512 * CREDIT - 1))
            .build()
            .expect_err("window below `max_num_streams * DEFAULT_CREDIT` must fail");

        assert_eq!(
            err,
            ConfigError::ReceiveWindowTooSmall {
                max_connection_receive_window: 512 * CREDIT - 1,
                max_num_streams: 512,
                min_required: 512 * CREDIT,
            }
        );
    }

    #[test]
    fn minimum_window_is_accepted() {
        let cfg = Config::builder()
            .max_num_streams(512)
            .max_connection_receive_window(Some(512 * CREDIT))
            .build()
            .expect("window of exactly `max_num_streams * DEFAULT_CREDIT` is valid");

        assert_eq!(cfg.max_num_streams, 512);
    }

    #[test]
    fn option_order_does_not_matter() {
        // `64 MiB`/`256` streams is a valid combination that a sequential,
        // validate-on-each-setter API would reject if the window was lowered
        // before the stream count (the still-default 512 streams need 128 MiB).
        let window = 256 * CREDIT;

        let window_first = Config::builder()
            .max_connection_receive_window(Some(window))
            .max_num_streams(256)
            .build();
        let streams_first = Config::builder()
            .max_num_streams(256)
            .max_connection_receive_window(Some(window))
            .build();

        assert!(window_first.is_ok());
        assert!(streams_first.is_ok());
    }

    #[test]
    fn unlimited_window_is_always_valid() {
        Config::builder()
            .max_num_streams(usize::MAX)
            .max_connection_receive_window(None)
            .build()
            .expect("`None` disables the limit and is valid for any stream count");
    }

    #[test]
    fn zero_streams_is_clamped_to_one() {
        let cfg = Config::builder()
            .max_num_streams(0)
            .build()
            .expect("valid config");

        assert_eq!(cfg.max_num_streams, 1);
    }
}
