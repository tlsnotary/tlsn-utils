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

use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use quickcheck::{Arbitrary, Gen};
use std::{
    io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
};
use tlsn_mux::{Config, Connection, ConnectionError};
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio_util::compat::{Compat, TokioAsyncReadCompatExt};

pub async fn connected_peers(
    server_config: Config,
    client_config: Config,
    buffer_sizes: Option<TcpBufferSizes>,
) -> io::Result<(Connection<Compat<TcpStream>>, Connection<Compat<TcpStream>>)> {
    let (listener, addr) = bind(buffer_sizes).await?;

    let server = async {
        let (stream, _) = listener.accept().await?;
        Ok(Connection::new(stream.compat(), server_config))
    };
    let client = async {
        let stream = new_socket(buffer_sizes)?.connect(addr).await?;
        Ok(Connection::new(stream.compat(), client_config))
    };

    futures::future::try_join(server, client).await
}

pub async fn bind(buffer_sizes: Option<TcpBufferSizes>) -> io::Result<(TcpListener, SocketAddr)> {
    let socket = new_socket(buffer_sizes)?;
    socket.bind(SocketAddr::V4(SocketAddrV4::new(
        Ipv4Addr::new(127, 0, 0, 1),
        0,
    )))?;

    let listener = socket.listen(1024)?;
    let address = listener.local_addr()?;

    Ok((listener, address))
}

fn new_socket(buffer_sizes: Option<TcpBufferSizes>) -> io::Result<TcpSocket> {
    let socket = TcpSocket::new_v4()?;
    if let Some(size) = buffer_sizes {
        socket.set_send_buffer_size(size.send)?;
        socket.set_recv_buffer_size(size.recv)?;
    }

    Ok(socket)
}

/// Send and receive buffer size for a TCP socket.
#[derive(Clone, Debug, Copy)]
pub struct TcpBufferSizes {
    send: u32,
    recv: u32,
}

impl Arbitrary for TcpBufferSizes {
    fn arbitrary(g: &mut Gen) -> Self {
        let send = if bool::arbitrary(g) {
            16 * 1024
        } else {
            32 * 1024
        };

        // Have receive buffer size be some multiple of send buffer size.
        let recv = if bool::arbitrary(g) {
            send * 2
        } else {
            send * 4
        };

        TcpBufferSizes { send, recv }
    }
}

pub async fn send_recv_message(stream: &mut tlsn_mux::Stream, Msg(msg): &Msg) -> io::Result<()> {
    let id = stream.id().to_vec();
    let (mut reader, mut writer) = AsyncReadExt::split(stream);

    let len = msg.len();
    let write_fut = async {
        writer.write_all(msg).await.unwrap();
        log::debug!("C: {id:?}: sent {len} bytes");
    };
    let mut data = vec![0; msg.len()];
    let read_fut = async {
        reader.read_exact(&mut data).await.unwrap();
        log::debug!("C: {:?}: received {} bytes", id, data.len());
    };
    futures::future::join(write_fut, read_fut).await;
    assert_eq!(&data, msg);

    Ok(())
}

/// Send all messages, using only a single stream.
pub async fn send_on_single_stream(
    mut stream: tlsn_mux::Stream,
    iter: impl IntoIterator<Item = Msg>,
) -> Result<(), ConnectionError> {
    log::debug!("C: new stream: {stream}");

    for msg in iter {
        send_recv_message(&mut stream, &msg).await?;
    }

    stream.close().await?;

    Ok(())
}

#[derive(Clone, Debug)]
pub struct Msg(pub Vec<u8>);

impl Arbitrary for Msg {
    fn arbitrary(g: &mut Gen) -> Msg {
        let mut msg = Msg(Arbitrary::arbitrary(g));
        if msg.0.is_empty() {
            msg.0.push(Arbitrary::arbitrary(g));
        }

        msg
    }

    fn shrink(&self) -> Box<dyn Iterator<Item = Self>> {
        Box::new(self.0.shrink().filter(|v| !v.is_empty()).map(Msg))
    }
}

#[derive(Clone, Debug)]
pub struct TestConfig(pub Config);

impl Arbitrary for TestConfig {
    fn arbitrary(g: &mut Gen) -> Self {
        use quickcheck::GenRange;

        let mut c = Config::default();
        let max_num_streams = 512;

        c.set_read_after_close(Arbitrary::arbitrary(g));
        c.set_max_num_streams(max_num_streams);
        if bool::arbitrary(g) {
            c.set_max_connection_receive_window(Some(
                g.gen_range(max_num_streams * (tlsn_mux::DEFAULT_CREDIT as usize)..usize::MAX),
            ));
        } else {
            c.set_max_connection_receive_window(None);
        }

        TestConfig(c)
    }
}

/// A server that reads from pre-registered streams and discards all data.
pub async fn dev_null_server<T>(mut conn: Connection<T>, nstreams: usize)
where
    T: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    // Pre-register streams with matching IDs
    let mut streams = Vec::with_capacity(nstreams);
    for i in 0..nstreams {
        let id = format!("stream-{i}");
        streams.push(conn.new_stream(id.as_bytes()).unwrap());
    }

    // Spawn connection poll loop
    tokio::spawn(async move {
        loop {
            if futures::future::poll_fn(|cx| conn.poll(cx)).await.is_ok() {
                break;
            }
        }
    });

    // Read and discard data from each stream
    let mut handles = Vec::new();
    for mut stream in streams {
        handles.push(tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            loop {
                match stream.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(_) => continue,
                    Err(_) => break,
                }
            }
        }));
    }

    for handle in handles {
        handle.await.ok();
    }
}

/// Strategy for sending messages.
#[derive(Debug, Clone, Copy)]
pub enum MessageSenderStrategy {
    /// Just send data without waiting for echo.
    Send,
}

/// Sends messages over multiple streams.
pub struct MessageSender<T> {
    conn: Connection<T>,
    messages: Vec<Msg>,
    message_multiplier: u64,
}

impl<T> MessageSender<T>
where
    T: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    pub fn new(conn: Connection<T>, messages: Vec<Msg>, _one_stream_per_message: bool) -> Self {
        Self {
            conn,
            messages,
            message_multiplier: 1,
        }
    }

    pub fn with_message_multiplier(mut self, multiplier: u64) -> Self {
        self.message_multiplier = multiplier;
        self
    }

    pub fn with_strategy(self, _strategy: MessageSenderStrategy) -> Self {
        self
    }

    pub async fn run(mut self) -> Result<usize, ConnectionError> {
        let stream_count = self.messages.len();

        // Create streams with matching IDs
        let mut streams = Vec::with_capacity(stream_count);
        for i in 0..stream_count {
            let id = format!("stream-{i}");
            streams.push(self.conn.new_stream(id.as_bytes())?);
        }

        // Spawn connection poll loop
        tokio::spawn(async move {
            loop {
                if futures::future::poll_fn(|cx| self.conn.poll(cx)).await.is_ok() {
                    break;
                }
            }
        });

        // Send messages
        let mut handles = Vec::new();
        for (mut stream, msg) in streams.into_iter().zip(self.messages) {
            let multiplier = self.message_multiplier;
            handles.push(tokio::spawn(async move {
                for _ in 0..multiplier {
                    if stream.write_all(&msg.0).await.is_err() {
                        break;
                    }
                }
                stream.close().await.ok();
            }));
        }

        for handle in handles {
            handle.await.ok();
        }

        Ok(stream_count)
    }
}

impl<T> std::future::IntoFuture for MessageSender<T>
where
    T: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    type Output = Result<usize, ConnectionError>;
    type IntoFuture = std::pin::Pin<Box<dyn std::future::Future<Output = Self::Output> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.run())
    }
}
