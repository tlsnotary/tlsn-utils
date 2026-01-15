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

use futures::{future, future::join, prelude::*, AsyncReadExt, AsyncWriteExt, FutureExt};
use quickcheck::QuickCheck;
use std::{panic::panic_any, pin::pin, time::Duration};
use test_harness::*;
use tlsn_mux::{Config, Connection, ConnectionError};
use tokio::{net::TcpStream, task, time::timeout};
use tokio_util::compat::TokioAsyncReadCompatExt;

#[test]
fn prop_config_send_recv_multi() {
    let _ = env_logger::try_init();

    async fn run_test(msgs: Vec<Msg>, cfg1: Config, cfg2: Config) {
        let num_messages = msgs.len();
        if num_messages == 0 {
            return;
        }

        let (listener, address) = bind(None).await.expect("bind");

        let server = async {
            let socket = listener.accept().await.expect("accept").0.compat();
            let mut connection = Connection::new(socket, cfg1);

            let mut streams = Vec::new();
            for i in 0..num_messages {
                let id = format!("stream-{}", i);
                streams.push(connection.get_stream(id.as_bytes()).unwrap());
            }

            task::spawn(async move {
                future::poll_fn(|cx| connection.poll(cx)).await.ok();
            });

            let tasks: Vec<_> = streams
                .into_iter()
                .map(|mut stream| {
                    task::spawn(async move {
                        {
                            let (mut r, mut w) = AsyncReadExt::split(&mut stream);
                            futures::io::copy(&mut r, &mut w).await?;
                        }
                        stream.close().await?;
                        Ok::<_, ConnectionError>(())
                    })
                })
                .collect();

            for task in tasks {
                task.await.unwrap().unwrap();
            }
        };

        let client = async {
            let socket = TcpStream::connect(address).await.expect("connect").compat();
            let mut connection = Connection::new(socket, cfg2);

            let mut streams = Vec::new();
            for i in 0..num_messages {
                let id = format!("stream-{}", i);
                streams.push(connection.get_stream(id.as_bytes()).unwrap());
            }

            task::spawn(async move {
                future::poll_fn(|cx| connection.poll(cx)).await.ok();
            });

            let tasks: Vec<_> = streams
                .into_iter()
                .zip(msgs)
                .map(|(mut stream, msg)| {
                    task::spawn(async move {
                        send_recv_message(&mut stream, &msg).await.unwrap();
                        stream.close().await.unwrap();
                    })
                })
                .collect();

            for task in tasks {
                task.await.unwrap();
            }
        };

        join(server, client).await;
    }

    fn prop(msgs: Vec<Msg>, TestConfig(cfg1): TestConfig, TestConfig(cfg2): TestConfig) {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(run_test(msgs, cfg1, cfg2));
    }

    QuickCheck::new().quickcheck(prop as fn(_, _, _) -> _)
}

#[test]
fn concurrent_streams() {
    let _ = env_logger::try_init();

    async fn run_test(tcp_buffer_sizes: Option<TcpBufferSizes>) {
        const PAYLOAD_SIZE: usize = 128 * 1024;

        let data = Msg(vec![0x42; PAYLOAD_SIZE]);
        let n_streams = 256;

        let mut cfg = Config::default();
        cfg.set_split_send_size(PAYLOAD_SIZE);
        cfg.set_max_num_streams(n_streams);

        let (mut server, mut client) = connected_peers(cfg.clone(), cfg, tcp_buffer_sizes)
            .await
            .unwrap();

        let mut server_streams = Vec::new();
        let mut client_streams = Vec::new();
        for i in 0..n_streams {
            let id = format!("stream-{}", i);
            server_streams.push(server.get_stream(id.as_bytes()).unwrap());
            client_streams.push(client.get_stream(id.as_bytes()).unwrap());
        }

        task::spawn(async move {
            future::poll_fn(|cx| server.poll(cx)).await.ok();
        });
        task::spawn(async move {
            future::poll_fn(|cx| client.poll(cx)).await.ok();
        });

        let server_tasks: Vec<_> = server_streams
            .into_iter()
            .map(|mut stream| {
                task::spawn(async move {
                    {
                        let (mut r, mut w) = AsyncReadExt::split(&mut stream);
                        futures::io::copy(&mut r, &mut w).await?;
                    }
                    stream.close().await?;
                    Ok::<_, ConnectionError>(())
                })
            })
            .collect();

        let client_tasks: Vec<_> = client_streams
            .into_iter()
            .map(|mut stream| {
                let msg = data.clone();
                task::spawn(async move {
                    send_recv_message(&mut stream, &msg).await.unwrap();
                    stream.close().await.unwrap();
                })
            })
            .collect();

        for task in client_tasks {
            task.await.unwrap();
        }
        for task in server_tasks {
            task.await.unwrap().unwrap();
        }
    }

    fn prop(tcp_buffer_sizes: Option<TcpBufferSizes>) {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(run_test(tcp_buffer_sizes));
    }

    QuickCheck::new().tests(3).quickcheck(prop as fn(_) -> _)
}

#[test]
fn prop_max_streams() {
    async fn run_test(n: usize) -> Result<bool, ConnectionError> {
        let max_streams = n % 100;
        if max_streams == 0 {
            return Ok(true);
        }
        let mut cfg = Config::default();
        cfg.set_max_num_streams(max_streams);

        let (mut server, mut client) = connected_peers(cfg.clone(), cfg.clone(), None).await?;

        for i in 0..max_streams {
            let id = format!("stream-{}", i);
            server.get_stream(id.as_bytes())?;
            client.get_stream(id.as_bytes())?;
        }

        task::spawn(async move {
            future::poll_fn(|cx| server.poll(cx)).await.ok();
        });
        task::spawn(async move {
            future::poll_fn(|cx| client.poll(cx)).await.ok();
        });

        let (mut _server2, mut client2) = connected_peers(cfg.clone(), cfg, None).await?;

        for i in 0..max_streams {
            let id = format!("stream-{}", i);
            client2.get_stream(id.as_bytes())?;
        }

        let extra_id = format!("stream-{}", max_streams);
        let result = client2.get_stream(extra_id.as_bytes());
        Ok(matches!(result, Err(ConnectionError::TooManyStreams)))
    }

    fn prop(n: usize) -> Result<bool, ConnectionError> {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(run_test(n))
    }

    QuickCheck::new().tests(7).quickcheck(prop as fn(_) -> _)
}

/// Test half-close: client sends FIN, server echoes and sends FIN, client reads
/// echo then EOF.
#[test]
fn half_closed() {
    let _ = env_logger::try_init();
    let stream_id = b"half-close";
    let message = b"echo me";

    let (server_endpoint, client_endpoint) = futures_ringbuf::Endpoint::pair(4096, 4096);
    let mut server = Connection::new(server_endpoint, Config::default());
    let mut client = Connection::new(client_endpoint, Config::default());

    let mut server_stream = server.get_stream(stream_id).unwrap();
    let mut client_stream = client.get_stream(stream_id).unwrap();

    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);

    // Client writes and closes (sends FIN)
    assert!(pin!(&mut client_stream)
        .poll_write(&mut cx, message)
        .is_ready());
    assert!(pin!(&mut client_stream).poll_close(&mut cx).is_ready());

    // Poll to exchange frames
    for _ in 0..20 {
        let _ = server.poll(&mut cx);
        let _ = client.poll(&mut cx);
    }

    // Server reads the data
    let mut buf = [0u8; 7];
    let result = pin!(&mut server_stream).poll_read(&mut cx, &mut buf);
    assert!(matches!(result, std::task::Poll::Ready(Ok(7))));
    assert_eq!(&buf, message);

    // Server reads EOF (FIN marker)
    let result = pin!(&mut server_stream).poll_read(&mut cx, &mut buf);
    assert!(matches!(result, std::task::Poll::Ready(Ok(0))));

    // Server echoes and closes
    assert!(pin!(&mut server_stream)
        .poll_write(&mut cx, message)
        .is_ready());
    assert!(pin!(&mut server_stream).poll_close(&mut cx).is_ready());

    // Poll to exchange frames
    for _ in 0..20 {
        let _ = server.poll(&mut cx);
        let _ = client.poll(&mut cx);
    }

    // Client reads echo
    let result = pin!(&mut client_stream).poll_read(&mut cx, &mut buf);
    assert!(matches!(result, std::task::Poll::Ready(Ok(7))));
    assert_eq!(&buf, message);

    // Client reads EOF (FIN marker)
    let result = pin!(&mut client_stream).poll_read(&mut cx, &mut buf);
    assert!(matches!(result, std::task::Poll::Ready(Ok(0))));
}

#[test]
fn prop_config_send_recv_single() {
    async fn run_test(
        mut msgs: Vec<Msg>,
        cfg1: Config,
        cfg2: Config,
    ) -> Result<(), ConnectionError> {
        msgs.insert(0, Msg(vec![1u8; tlsn_mux::DEFAULT_CREDIT as usize]));
        let stream_id = b"single-stream";

        let (mut server, mut client) = connected_peers(cfg1, cfg2, None).await?;

        let mut server_stream = server.get_stream(stream_id)?;
        let client_stream = client.get_stream(stream_id)?;

        task::spawn(async move {
            future::poll_fn(|cx| server.poll(cx)).await.ok();
        });
        task::spawn(async move {
            future::poll_fn(|cx| client.poll(cx)).await.ok();
        });

        let server_task = task::spawn(async move {
            {
                let (mut r, mut w) = AsyncReadExt::split(&mut server_stream);
                futures::io::copy(&mut r, &mut w).await?;
            }
            server_stream.close().await?;
            Ok::<_, ConnectionError>(())
        });

        send_on_single_stream(client_stream, msgs).await?;
        server_task.await.unwrap()?;
        Ok(())
    }

    fn prop(msgs: Vec<Msg>, TestConfig(cfg1): TestConfig, TestConfig(cfg2): TestConfig) {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(run_test(msgs, cfg1, cfg2))
            .unwrap();
    }

    QuickCheck::new()
        .tests(10)
        .quickcheck(prop as fn(_, _, _) -> _)
}

#[tokio::test(flavor = "multi_thread")]
async fn write_deadlock() {
    let _ = env_logger::try_init();

    let msg = vec![1u8; 1024 * 1024];
    let stream_id = b"deadlock-test";

    let (server_endpoint, client_endpoint) = futures_ringbuf::Endpoint::pair(1024, 1024);

    let mut server = Connection::new(server_endpoint, Config::default());
    let mut server_stream = server.get_stream(stream_id).unwrap();
    let mut client = Connection::new(client_endpoint, Config::default());
    let client_stream = client.get_stream(stream_id).unwrap();

    task::spawn(async move {
        futures::select_biased! {
            _ = async {
                {
                    let (mut r, mut w) = AsyncReadExt::split(&mut server_stream);
                    futures::io::copy(&mut r, &mut w).await.unwrap();
                }
                server_stream.close().await.unwrap();
            }.fuse() => {},
            _ = async { future::poll_fn(|cx| server.poll(cx)).await.ok(); }.fuse() => {},
        }
    });

    task::spawn(async move {
        future::poll_fn(|cx| client.poll(cx)).await.ok();
    });

    timeout(Duration::from_secs(10), async {
        let (mut reader, mut writer) = AsyncReadExt::split(client_stream);
        let mut buf = vec![0; msg.len()];
        let _ = join(
            writer.write_all(&msg).map_err(|e| panic_any(e)),
            reader.read_exact(&mut buf).map_err(|e| panic_any(e)),
        )
        .await;
        let mut stream = reader.reunite(writer).unwrap();
        stream.close().await.unwrap();
        assert_eq!(buf, msg);
    })
    .await
    .expect("timeout");
}

/// Test that data written before dropping a stream handle is still delivered.
/// Note: With the simplified protocol, dropping does NOT send FIN to remote.
#[test]
fn drop_delivers_written_data() {
    let _ = env_logger::try_init();
    let stream_id = b"drop-test";

    let (server_endpoint, client_endpoint) = futures_ringbuf::Endpoint::pair(1024, 1024);
    let mut server = Connection::new(server_endpoint, Config::default());
    let mut client = Connection::new(client_endpoint, Config::default());

    let mut server_stream = server.get_stream(stream_id).unwrap();
    let client_stream = client.get_stream(stream_id).unwrap();

    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);

    // Client writes and drops (no close/FIN)
    assert!(pin!(client_stream).poll_write(&mut cx, &[42]).is_ready());
    // stream dropped here

    // Poll to deliver data
    for _ in 0..10 {
        let _ = server.poll(&mut cx);
        let _ = client.poll(&mut cx);
    }

    // Server should receive the data
    let mut buf = [0u8; 1];
    let result = pin!(&mut server_stream).poll_read(&mut cx, &mut buf);
    assert!(matches!(result, std::task::Poll::Ready(Ok(1))));
    assert_eq!(buf[0], 42);

    // Server does NOT get EOF (no FIN was sent)
    let result = pin!(&mut server_stream).poll_read(&mut cx, &mut buf);
    assert!(result.is_pending());
}

#[test]
fn close_sync() {
    let _ = env_logger::try_init();

    let (server_endpoint, client_endpoint) = futures_ringbuf::Endpoint::pair(1024, 1024);
    let mut config = Config::default();
    config.set_close_sync(true);
    let mut server = Connection::new(server_endpoint, config.clone());
    let mut client = Connection::new(client_endpoint, config);

    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);

    let stream_id = b"test";
    let client_stream = client.get_stream(stream_id).unwrap();
    let server_stream = server.get_stream(stream_id).unwrap();

    assert!(pin!(client_stream).poll_write(&mut cx, b"hello").is_ready());
    client.close();

    for _ in 0..10 {
        assert!(client.poll(&mut cx).is_pending());
    }

    let _ = server.poll(&mut cx);
    let _ = server.poll(&mut cx);

    let mut buf = [0u8; 5];
    assert!(pin!(server_stream).poll_read(&mut cx, &mut buf).is_ready());
    assert_eq!(&buf, b"hello");

    while server.poll(&mut cx).is_pending() {}

    server.close();
    let _ = server.poll(&mut cx);

    while client.poll(&mut cx).is_pending() {}
}

/// Test that after dropping a stream handle, the stream can be reopened
/// and any data buffered during the "closed" period can be read.
#[test]
fn stream_reuse_after_handle_drop() {
    let _ = env_logger::try_init();
    let stream_id = b"reuse-test";
    let message = b"buffered data";

    let (server_endpoint, client_endpoint) = futures_ringbuf::Endpoint::pair(4096, 4096);
    let mut server = Connection::new(server_endpoint, Config::default());
    let mut client = Connection::new(client_endpoint, Config::default());

    let mut server_stream = server.get_stream(stream_id).unwrap();
    let client_stream = client.get_stream(stream_id).unwrap();

    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);

    // Drop client stream handle
    drop(client_stream);

    // Server writes data
    assert!(pin!(&mut server_stream)
        .poll_write(&mut cx, message)
        .is_ready());
    // Server sends FIN
    assert!(pin!(&mut server_stream).poll_close(&mut cx).is_ready());

    // Poll both connections to exchange frames
    for _ in 0..20 {
        let _ = server.poll(&mut cx);
        let _ = client.poll(&mut cx);
    }

    // Reopen stream on client
    let mut reopened = client.get_stream(stream_id).unwrap();

    // Read buffered data
    let mut buf = [0u8; 14];
    let result = pin!(&mut reopened).poll_read(&mut cx, &mut buf);
    assert!(result.is_ready());
    if let std::task::Poll::Ready(Ok(n)) = result {
        assert_eq!(n, message.len());
        assert_eq!(&buf[..n], message);
    }

    // Read FIN marker (EOF)
    let result = pin!(&mut reopened).poll_read(&mut cx, &mut buf);
    assert!(matches!(result, std::task::Poll::Ready(Ok(0))));
}

/// Test that data can be sent after FIN (FIN is just an in-band marker).
#[test]
fn data_after_fin() {
    let _ = env_logger::try_init();
    let stream_id = b"data-after-fin";

    let (server_endpoint, client_endpoint) = futures_ringbuf::Endpoint::pair(4096, 4096);
    let mut server = Connection::new(server_endpoint, Config::default());
    let mut client = Connection::new(client_endpoint, Config::default());

    let mut server_stream = server.get_stream(stream_id).unwrap();
    let mut client_stream = client.get_stream(stream_id).unwrap();

    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);

    // Client sends data
    assert!(pin!(&mut client_stream)
        .poll_write(&mut cx, b"before")
        .is_ready());

    // Client sends FIN
    assert!(pin!(&mut client_stream).poll_close(&mut cx).is_ready());

    // Client sends more data AFTER FIN
    assert!(pin!(&mut client_stream)
        .poll_write(&mut cx, b"after")
        .is_ready());

    // Poll to exchange frames
    for _ in 0..20 {
        let _ = server.poll(&mut cx);
        let _ = client.poll(&mut cx);
    }

    // Server reads "before"
    let mut buf = [0u8; 6];
    let result = pin!(&mut server_stream).poll_read(&mut cx, &mut buf);
    assert!(matches!(result, std::task::Poll::Ready(Ok(6))));
    assert_eq!(&buf, b"before");

    // Server reads EOF (FIN marker)
    let result = pin!(&mut server_stream).poll_read(&mut cx, &mut buf);
    assert!(matches!(result, std::task::Poll::Ready(Ok(0))));

    // Server reads "after" (data sent after FIN)
    let mut buf = [0u8; 5];
    let result = pin!(&mut server_stream).poll_read(&mut cx, &mut buf);
    assert!(matches!(result, std::task::Poll::Ready(Ok(5))));
    assert_eq!(&buf, b"after");
}

/// Test that streams can be reused multiple times on the same connection.
#[test]
fn stream_reuse_same_connection() {
    let _ = env_logger::try_init();
    let stream_id = b"reuse-multi";

    let (server_endpoint, client_endpoint) = futures_ringbuf::Endpoint::pair(4096, 4096);
    let mut server = Connection::new(server_endpoint, Config::default());
    let mut client = Connection::new(client_endpoint, Config::default());

    let mut server_stream = server.get_stream(stream_id).unwrap();

    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);

    // First use: create, write, drop (no FIN sent)
    let stream = client.get_stream(stream_id).unwrap();
    assert!(pin!(stream).poll_write(&mut cx, b"first").is_ready());
    // stream dropped here

    // Poll to deliver data
    for _ in 0..10 {
        let _ = server.poll(&mut cx);
        let _ = client.poll(&mut cx);
    }

    // Reopen same stream ID and write more
    let stream = client.get_stream(stream_id).unwrap();
    assert!(pin!(stream).poll_write(&mut cx, b"second").is_ready());

    // Poll to deliver data
    for _ in 0..10 {
        let _ = server.poll(&mut cx);
        let _ = client.poll(&mut cx);
    }

    // Server should receive both messages
    let mut buf = [0u8; 5];
    let result = pin!(&mut server_stream).poll_read(&mut cx, &mut buf);
    assert!(matches!(result, std::task::Poll::Ready(Ok(5))));
    assert_eq!(&buf, b"first");

    let mut buf = [0u8; 6];
    let result = pin!(&mut server_stream).poll_read(&mut cx, &mut buf);
    assert!(matches!(result, std::task::Poll::Ready(Ok(6))));
    assert_eq!(&buf, b"second");
}
