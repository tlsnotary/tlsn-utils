//! Test utilities.

use tokio::io::{duplex, DuplexStream};
use tokio_util::compat::{Compat, TokioAsyncReadCompatExt};
use yamux::{Config, Mode};

use crate::{
    yamux::{Yamux, YamuxCtrl},
    FramedMux,
};

/// Creates a test pair of yamux instances.
///
/// # Arguments
///
/// * `buffer` - The buffer size.
pub fn test_yamux_pair(
    buffer: usize,
) -> (Yamux<Compat<DuplexStream>>, Yamux<Compat<DuplexStream>>) {
    let (a, b) = duplex(buffer);

    let a = Yamux::new(a.compat(), Config::default(), Mode::Client);
    let b = Yamux::new(b.compat(), Config::default(), Mode::Server);

    (a, b)
}

/// Creates a test pair of framed yamux instances.
///
/// # Arguments
///
/// * `buffer` - The buffer size.
/// * `codec` - The codec.
pub fn test_yamux_pair_framed<C: Clone>(
    buffer: usize,
    codec: C,
) -> (
    (FramedMux<YamuxCtrl, C>, Yamux<Compat<DuplexStream>>),
    (FramedMux<YamuxCtrl, C>, Yamux<Compat<DuplexStream>>),
) {
    let (a, b) = test_yamux_pair(buffer);

    let ctrl_a = FramedMux::new(a.control(), codec.clone());
    let ctrl_b = FramedMux::new(b.control(), codec);

    ((ctrl_a, a), (ctrl_b, b))
}
