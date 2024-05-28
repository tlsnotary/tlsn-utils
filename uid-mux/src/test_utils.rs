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

#[cfg(feature = "serio")]
mod serio {
    use core::fmt;
    use std::{
        collections::{HashMap, HashSet},
        sync::{Arc, Mutex},
    };

    use async_trait::async_trait;
    use serio::channel::{duplex, MemoryDuplex};

    use crate::serio::FramedUidMux;

    /// Error for [`TestFramedMux`].
    #[derive(Debug)]
    pub struct TestFramedMuxError(&'static str);

    impl fmt::Display for TestFramedMuxError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    impl std::error::Error for TestFramedMuxError {}

    #[derive(Debug, Default)]
    struct State {
        exists: HashSet<Vec<u8>>,
        waiting_a: HashMap<Vec<u8>, MemoryDuplex>,
        waiting_b: HashMap<Vec<u8>, MemoryDuplex>,
    }

    #[derive(Debug, Clone, Copy)]
    enum Role {
        A,
        B,
    }

    /// A test framed mux.
    #[derive(Debug, Clone)]
    pub struct TestFramedMux {
        role: Role,
        buffer: usize,
        state: Arc<Mutex<State>>,
    }

    #[async_trait]
    impl<Id: AsRef<[u8]> + Send + Sync> FramedUidMux<Id> for TestFramedMux {
        /// Stream type.
        type Framed = MemoryDuplex;
        /// Error type.
        type Error = TestFramedMuxError;

        /// Opens a new framed stream with the given id.
        async fn open_framed(&self, id: &Id) -> Result<Self::Framed, Self::Error> {
            let mut state = self.state.lock().unwrap();

            if let Some(channel) = match self.role {
                Role::A => state.waiting_a.remove(id.as_ref()),
                Role::B => state.waiting_b.remove(id.as_ref()),
            } {
                Ok(channel)
            } else {
                if !state.exists.insert(id.as_ref().to_vec()) {
                    return Err(TestFramedMuxError("duplicate stream id"));
                }

                let (a, b) = duplex(self.buffer);

                match self.role {
                    Role::A => {
                        state.waiting_b.insert(id.as_ref().to_vec(), b);
                        Ok(a)
                    }
                    Role::B => {
                        state.waiting_a.insert(id.as_ref().to_vec(), a);
                        Ok(b)
                    }
                }
            }
        }
    }

    /// Creates a test pair of framed mux instances.
    pub fn test_framed_mux(buffer: usize) -> (TestFramedMux, TestFramedMux) {
        let state = Arc::new(Mutex::new(State::default()));

        (
            TestFramedMux {
                role: Role::A,
                buffer,
                state: state.clone(),
            },
            TestFramedMux {
                role: Role::B,
                buffer,
                state,
            },
        )
    }

    #[cfg(test)]
    mod tests {
        use serio::{SinkExt, StreamExt};

        use super::*;

        #[test]
        fn test_framed_mux() {
            let (a, b) = super::test_framed_mux(1);

            futures::executor::block_on(async {
                let mut a_0 = a.open_framed(&[0]).await.unwrap();
                let mut b_0 = b.open_framed(&[0]).await.unwrap();

                let mut a_1 = a.open_framed(&[1]).await.unwrap();
                let mut b_1 = b.open_framed(&[1]).await.unwrap();

                a_0.send(42u8).await.unwrap();
                assert_eq!(b_0.next::<u8>().await.unwrap().unwrap(), 42);

                a_1.send(69u8).await.unwrap();
                assert_eq!(b_1.next::<u8>().await.unwrap().unwrap(), 69u8);
            })
        }

        #[test]
        fn test_framed_mux_duplicate() {
            let (a, b) = super::test_framed_mux(1);

            futures::executor::block_on(async {
                let _ = a.open_framed(&[0]).await.unwrap();
                let _ = b.open_framed(&[0]).await.unwrap();

                assert!(a.open_framed(&[0]).await.is_err());
                assert!(b.open_framed(&[0]).await.is_err());
            })
        }
    }
}

#[cfg(feature = "serio")]
pub use serio::*;
