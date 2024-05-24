use ::serio::{codec::Codec, IoDuplex};
use async_trait::async_trait;

use crate::UidMux;

/// A multiplexer that opens framed streams with unique ids.
#[async_trait]
pub trait FramedUidMux<Id> {
    /// Stream type.
    type Framed: IoDuplex;
    /// Error type.
    type Error;

    /// Opens a new framed stream with the given id.
    async fn open_framed(&self, id: &Id) -> Result<Self::Framed, Self::Error>;
}

/// A framed multiplexer.
#[derive(Debug)]
pub struct FramedMux<M, C> {
    mux: M,
    codec: C,
}

impl<M, C> FramedMux<M, C> {
    /// Creates a new `FramedMux`.
    pub fn new(mux: M, codec: C) -> Self {
        Self { mux, codec }
    }

    /// Returns a reference to the mux.
    pub fn mux(&self) -> &M {
        &self.mux
    }

    /// Returns a mutable reference to the mux.
    pub fn mux_mut(&mut self) -> &mut M {
        &mut self.mux
    }

    /// Returns a reference to the codec.
    pub fn codec(&self) -> &C {
        &self.codec
    }

    /// Returns a mutable reference to the codec.
    pub fn codec_mut(&mut self) -> &mut C {
        &mut self.codec
    }

    /// Splits the `FramedMux` into its parts.
    pub fn into_parts(self) -> (M, C) {
        (self.mux, self.codec)
    }
}

#[async_trait]
impl<Id, M, C> FramedUidMux<Id> for FramedMux<M, C>
where
    Id: Sync,
    M: UidMux<Id> + Sync,
    C: Codec<<M as UidMux<Id>>::Stream> + Sync,
{
    /// Stream type.
    type Framed = <C as Codec<<M as UidMux<Id>>::Stream>>::Framed;
    /// Error type.
    type Error = <M as UidMux<Id>>::Error;

    /// Opens a new framed stream with the given id.
    async fn open_framed(&self, id: &Id) -> Result<Self::Framed, Self::Error> {
        let stream = self.mux.open(id).await?;
        Ok(self.codec.new_framed(stream))
    }
}

impl<M: Clone, C: Clone> Clone for FramedMux<M, C> {
    fn clone(&self) -> Self {
        Self {
            mux: self.mux.clone(),
            codec: self.codec.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::IntoFuture;

    use super::*;
    use crate::yamux::{Config, Mode, Yamux};

    use ::serio::codec::Bincode;
    use serio::{stream::IoStreamExt, SinkExt};
    use tokio::io::duplex;
    use tokio_util::compat::TokioAsyncReadCompatExt;

    #[tokio::test]
    async fn test_framed_mux() {
        let (client_io, server_io) = duplex(1024);
        let client = Yamux::new(client_io.compat(), Config::default(), Mode::Client);
        let server = Yamux::new(server_io.compat(), Config::default(), Mode::Server);

        let client_ctrl = FramedMux::new(client.control(), Bincode);
        let server_ctrl = FramedMux::new(server.control(), Bincode);

        let conn_task = tokio::spawn(async {
            futures::try_join!(client.into_future(), server.into_future()).unwrap();
        });

        futures::join!(
            async {
                let mut stream = client_ctrl.open_framed(b"test").await.unwrap();

                stream.send(42u128).await.unwrap();

                client_ctrl.mux().close();
            },
            async {
                let mut stream = server_ctrl.open_framed(b"test").await.unwrap();

                let num: u128 = stream.expect_next().await.unwrap();

                server_ctrl.mux().close();

                assert_eq!(num, 42u128);
            }
        );

        conn_task.await.unwrap();
    }
}
