// Copyright (c) 2018-2019 Parity Technologies (UK) Ltd.
// Modifications Copyright (c) 2025 TLSNotary
//
// Licensed under the Apache License, Version 2.0 or MIT license, at your
// option.
//
// A copy of the Apache License, Version 2.0 is included in the software as
// LICENSE-APACHE and a copy of the MIT license is included in the software
// as LICENSE-MIT. You may also obtain a copy of the Apache License, Version 2.0
// at https://www.apache.org/licenses/LICENSE-2.0 and a copy of the MIT license
// at https://opensource.org/licenses/MIT.

#![allow(clippy::all)]

use futures::{
    future,
    future::{BoxFuture, FutureExt},
    AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt,
};
use std::{
    future::Future,
    mem,
    pin::Pin,
    task::{Context, Poll},
};
use test_harness::bind;
use tlsn_mux::{Config, Connection, ConnectionError, Mode, Stream};
use tokio::net::TcpStream;
use tokio_util::compat::TokioAsyncReadCompatExt;

#[tokio::test]
async fn stream_is_acknowledged_on_first_use() {
    let _ = env_logger::try_init();

    let stream_id = b"test-stream";

    let (listener, address) = bind(None).await.expect("bind");

    let server = async {
        let socket = listener.accept().await.expect("accept").0.compat();
        let connection = Connection::new(socket, Config::default(), Mode::Server);

        Server::new(connection, stream_id).await
    };

    let client = async {
        let socket = TcpStream::connect(address).await.expect("connect").compat();
        let connection = Connection::new(socket, Config::default(), Mode::Client);

        Client::new(connection, stream_id).await
    };

    let ((), ()) = future::try_join(server, client).await.unwrap();
}

enum Server<T> {
    Working {
        connection: Connection<T>,
        stream: BoxFuture<'static, tlsn_mux::Result<()>>,
    },
    Idle {
        connection: Connection<T>,
    },
    Poisoned,
}

impl<T> Server<T>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    fn new(mut connection: Connection<T>, stream_id: &[u8]) -> Self {
        let stream = connection.new_stream(stream_id).expect("new_stream");
        Server::Working {
            connection,
            stream: pong_ping(stream).boxed(),
        }
    }
}

impl<T> Future for Server<T>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    type Output = tlsn_mux::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        loop {
            match mem::replace(this, Self::Poisoned) {
                Self::Working {
                    mut connection,
                    mut stream,
                } => {
                    match stream.poll_unpin(cx)? {
                        Poll::Ready(()) => {
                            *this = Self::Idle { connection };
                            continue;
                        }
                        Poll::Pending => {}
                    }

                    match connection.poll(cx) {
                        Poll::Ready(Ok(())) => {
                            return Poll::Ready(Ok(()));
                        }
                        Poll::Ready(Err(ConnectionError::Closed)) => {
                            return Poll::Ready(Ok(()));
                        }
                        Poll::Ready(Err(e)) => {
                            return Poll::Ready(Err(e));
                        }
                        Poll::Pending => {
                            *this = Self::Working { connection, stream };
                            return Poll::Pending;
                        }
                    }
                }
                Self::Idle { mut connection } => match connection.poll(cx) {
                    Poll::Ready(Ok(())) => return Poll::Ready(Ok(())),
                    Poll::Ready(Err(ConnectionError::Closed)) => return Poll::Ready(Ok(())),
                    Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                    Poll::Pending => {
                        *this = Self::Idle { connection };
                        return Poll::Pending;
                    }
                },
                Self::Poisoned => unreachable!(),
            }
        }
    }
}

enum Client<T> {
    Working {
        connection: Connection<T>,
        stream: BoxFuture<'static, tlsn_mux::Result<()>>,
    },
    Poisoned,
}

impl<T> Client<T>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    fn new(mut connection: Connection<T>, stream_id: &[u8]) -> Self {
        let stream = connection.new_stream(stream_id).expect("new_stream");
        Self::Working {
            connection,
            stream: ping_pong(stream).boxed(),
        }
    }
}

impl<T> Future for Client<T>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    type Output = tlsn_mux::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        loop {
            match mem::replace(this, Self::Poisoned) {
                Self::Working {
                    mut connection,
                    mut stream,
                } => {
                    match stream.poll_unpin(cx)? {
                        Poll::Ready(()) => {
                            return Poll::Ready(Ok(()));
                        }
                        Poll::Pending => {}
                    }

                    match connection.poll(cx) {
                        Poll::Ready(Ok(())) => {
                            return Poll::Ready(Ok(()));
                        }
                        Poll::Ready(Err(ConnectionError::Closed)) => {
                            return Poll::Ready(Ok(()));
                        }
                        Poll::Ready(Err(e)) => {
                            return Poll::Ready(Err(e));
                        }
                        Poll::Pending => {
                            *this = Self::Working { connection, stream };
                            return Poll::Pending;
                        }
                    }
                }
                Self::Poisoned => unreachable!(),
            }
        }
    }
}

/// Handler for the **outbound** stream on the client.
///
/// Initially, the stream is not acknowledged. The server will only acknowledge
/// the stream with the first frame.
async fn ping_pong(mut stream: Stream) -> Result<(), ConnectionError> {
    assert!(
        stream.is_pending_ack(),
        "newly returned stream should not be acknowledged"
    );

    let mut buffer = [0u8; 4];
    stream.write_all(b"ping").await?;
    stream.read_exact(&mut buffer).await?;

    assert!(
        !stream.is_pending_ack(),
        "stream should be acknowledged once we received the first data"
    );
    assert_eq!(&buffer, b"pong");

    stream.close().await?;

    Ok(())
}

/// Handler for the **inbound** stream on the server.
///
/// Initially, the stream is not acknowledged. We only include the ACK flag in
/// the first frame.
async fn pong_ping(mut stream: Stream) -> Result<(), ConnectionError> {
    assert!(
        stream.is_pending_ack(),
        "before sending anything we should not have acknowledged the stream to the remote"
    );

    let mut buffer = [0u8; 4];
    stream.write_all(b"pong").await?;

    assert!(
        !stream.is_pending_ack(),
        "we should have sent an ACK flag with the first payload"
    );

    stream.read_exact(&mut buffer).await?;

    assert_eq!(&buffer, b"ping");

    stream.close().await?;

    Ok(())
}
