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

use futures::{
    executor::LocalPool,
    future,
    future::join,
    prelude::*,
    task::{Spawn, SpawnExt},
    AsyncReadExt, AsyncWriteExt, FutureExt,
};
use quickcheck::QuickCheck;
use std::{panic::panic_any, pin::pin};
use test_harness::*;
use tlsn_mux::{Config, Connection, ConnectionError, Mode};
use tokio::{net::TcpStream, task};
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
            let mut connection = Connection::new(socket, cfg1, Mode::Server);

            // Pre-register streams
            let mut streams = Vec::new();
            for i in 0..num_messages {
                let id = format!("stream-{}", i);
                streams.push(connection.new_stream(id.as_bytes()).unwrap());
            }

            // Spawn connection poll loop
            task::spawn(async move {
                future::poll_fn(|cx| connection.poll(cx)).await.ok();
            });

            // Echo each stream
            let mut tasks = Vec::new();
            for mut stream in streams {
                tasks.push(task::spawn(async move {
                    {
                        let (mut r, mut w) = AsyncReadExt::split(&mut stream);
                        futures::io::copy(&mut r, &mut w).await?;
                    }
                    stream.close().await?;
                    Ok::<_, ConnectionError>(())
                }));
            }

            for task in tasks {
                task.await.unwrap().unwrap();
            }
        };

        let client = async {
            let socket = TcpStream::connect(address).await.expect("connect").compat();
            let mut connection = Connection::new(socket, cfg2, Mode::Client);

            // Create streams
            let mut streams = Vec::new();
            for i in 0..num_messages {
                let id = format!("stream-{}", i);
                streams.push(connection.new_stream(id.as_bytes()).unwrap());
            }

            // Spawn connection poll loop
            task::spawn(async move {
                future::poll_fn(|cx| connection.poll(cx)).await.ok();
            });

            // Send/recv on each stream
            let mut tasks = Vec::new();
            for (stream, msg) in streams.into_iter().zip(msgs.into_iter()) {
                tasks.push(task::spawn(async move {
                    let mut stream = stream;
                    send_recv_message(&mut stream, &msg).await.unwrap();
                    stream.close().await.unwrap();
                }));
            }

            for task in tasks {
                task.await.unwrap();
            }
        };

        futures::future::join(server, client).await;
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

        // Pre-register streams on server
        let mut server_streams = Vec::new();
        for i in 0..n_streams {
            let id = format!("stream-{}", i);
            server_streams.push(server.new_stream(id.as_bytes()).unwrap());
        }

        // Create streams on client
        let mut client_streams = Vec::new();
        for i in 0..n_streams {
            let id = format!("stream-{}", i);
            client_streams.push(client.new_stream(id.as_bytes()).unwrap());
        }

        // Spawn connection poll loops
        task::spawn(async move {
            future::poll_fn(|cx| server.poll(cx)).await.ok();
        });
        task::spawn(async move {
            future::poll_fn(|cx| client.poll(cx)).await.ok();
        });

        // Server echoes
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

        // Client send/recv
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

        // Wait for all client tasks
        for task in client_tasks {
            task.await.unwrap();
        }

        // Wait for all server tasks
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
            return Ok(true); // Skip zero streams
        }
        let mut cfg = Config::default();
        cfg.set_max_num_streams(max_streams);

        let (mut server, mut client) = connected_peers(cfg.clone(), cfg.clone(), None).await?;

        // Pre-register streams on server
        for i in 0..max_streams {
            let id = format!("stream-{}", i);
            server.new_stream(id.as_bytes())?;
        }

        // Create streams on client
        for i in 0..max_streams {
            let id = format!("stream-{}", i);
            client.new_stream(id.as_bytes())?;
        }

        // Spawn connection poll loops
        task::spawn(async move {
            future::poll_fn(|cx| server.poll(cx)).await.ok();
        });
        task::spawn(async move {
            future::poll_fn(|cx| client.poll(cx)).await.ok();
        });

        // Can't open more on a fresh connection since we've already created max streams
        // But we need a fresh connection to test this
        let (mut _server2, mut client2) = connected_peers(cfg.clone(), cfg, None).await?;

        // Open max_streams on client2
        for i in 0..max_streams {
            let id = format!("stream-{}", i);
            client2.new_stream(id.as_bytes())?;
        }

        // Try to open one more stream - should fail
        let extra_id = format!("stream-{}", max_streams);
        let open_result = client2.new_stream(extra_id.as_bytes());
        Ok(matches!(open_result, Err(ConnectionError::TooManyStreams)))
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

#[test]
fn prop_send_recv_half_closed() {
    async fn run_test(msg: Msg) -> Result<(), ConnectionError> {
        let msg_len = msg.0.len();
        let stream_id = b"test-stream";

        let (mut server, mut client) =
            connected_peers(Config::default(), Config::default(), None).await?;

        // Create streams before spawning connections
        let mut server_stream = server.new_stream(stream_id)?;
        let mut client_stream = client.new_stream(stream_id)?;

        // Spawn connection poll loops
        task::spawn(async move {
            future::poll_fn(|cx| server.poll(cx)).await.ok();
        });
        task::spawn(async move {
            future::poll_fn(|cx| client.poll(cx)).await.ok();
        });

        // Server echoes back
        let server_task = task::spawn(async move {
            let mut buf = vec![0; msg_len];
            server_stream.read_exact(&mut buf).await?;
            server_stream.write_all(&buf).await?;
            server_stream.close().await?;
            Ok::<_, ConnectionError>(())
        });

        // Client writes, closes, then reads response
        client_stream.write_all(&msg.0).await?;
        client_stream.close().await?;

        assert!(client_stream.is_write_closed());
        let mut buf = vec![0; msg_len];
        client_stream.read_exact(&mut buf).await?;

        assert_eq!(buf, msg.0);
        assert_eq!(Some(0), client_stream.read(&mut buf).await.ok());
        assert!(client_stream.is_closed());

        server_task.await.unwrap()?;

        Ok(())
    }

    fn prop(msg: Msg) {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(run_test(msg))
            .unwrap();
    }

    QuickCheck::new().tests(7).quickcheck(prop as fn(_) -> _)
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

        // Create streams before spawning connections
        let mut server_stream = server.new_stream(stream_id)?;
        let client_stream = client.new_stream(stream_id)?;

        // Spawn connection poll loops
        task::spawn(async move {
            future::poll_fn(|cx| server.poll(cx)).await.ok();
        });
        task::spawn(async move {
            future::poll_fn(|cx| client.poll(cx)).await.ok();
        });

        // Server echoes
        let server_task = task::spawn(async move {
            {
                let (mut r, mut w) = AsyncReadExt::split(&mut server_stream);
                futures::io::copy(&mut r, &mut w).await?;
            }
            server_stream.close().await?;
            Ok::<_, ConnectionError>(())
        });

        // Client sends all messages
        send_on_single_stream(client_stream, msgs).await?;

        server_task.await.unwrap()?;

        Ok(())
    }

    fn prop(msgs: Vec<Msg>, TestConfig(cfg1): TestConfig, TestConfig(cfg2): TestConfig) {
        // Use multi-threaded runtime so task::spawn works
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

/// This test simulates two endpoints of a multiplexer connection which may be
/// unable to write simultaneously but can make progress by reading.
#[test]
fn write_deadlock() {
    let _ = env_logger::try_init();
    let mut pool = LocalPool::new();

    let msg = vec![1u8; 1024 * 1024];
    let capacity = 1024;
    let stream_id = b"deadlock-test";

    let (server_endpoint, client_endpoint) = futures_ringbuf::Endpoint::pair(capacity, capacity);

    // Create and spawn a "server" that echoes every message back to the client.
    let mut server = Connection::new(server_endpoint, Config::default(), Mode::Server);
    let server_stream = server.new_stream(stream_id).unwrap();
    pool.spawner()
        .spawn_obj(
            async move {
                let mut stream = server_stream;
                let conn_task = async {
                    loop {
                        if future::poll_fn(|cx| server.poll(cx)).await.is_ok() {
                            break;
                        }
                    }
                };
                let echo_task = async {
                    {
                        let (mut r, mut w) = AsyncReadExt::split(&mut stream);
                        futures::io::copy(&mut r, &mut w).await.unwrap();
                    }
                    stream.close().await.unwrap();
                };
                futures::select_biased! {
                    _ = echo_task.fuse() => {},
                    _ = conn_task.fuse() => {},
                }
            }
            .boxed()
            .into(),
        )
        .unwrap();

    // Create and spawn a "client"
    let mut client = Connection::new(client_endpoint, Config::default(), Mode::Client);
    let stream = client.new_stream(stream_id).unwrap();

    // Continuously advance the multiplexer connection of the client
    pool.spawner()
        .spawn_obj(
            async move {
                loop {
                    if future::poll_fn(|cx| client.poll(cx)).await.is_ok() {
                        break;
                    }
                }
            }
            .boxed()
            .into(),
        )
        .unwrap();

    // Send the message, expecting it to be echo'd.
    pool.run_until(
        pool.spawner()
            .spawn_with_handle(
                async move {
                    let (mut reader, mut writer) = AsyncReadExt::split(stream);
                    let mut b = vec![0; msg.len()];
                    let _ = join(
                        writer.write_all(msg.as_ref()).map_err(|e| panic_any(e)),
                        reader.read_exact(&mut b[..]).map_err(|e| panic_any(e)),
                    )
                    .await;
                    let mut stream = reader.reunite(writer).unwrap();
                    stream.close().await.unwrap();
                    log::debug!("C: Stream {:?} done.", stream.id());
                    assert_eq!(b, msg);
                }
                .boxed(),
            )
            .unwrap(),
    );
}

#[test]
fn close_through_drop_of_stream_propagates_to_remote() {
    let _ = env_logger::try_init();
    let mut pool = LocalPool::new();
    let stream_id = b"drop-test";

    let (server_endpoint, client_endpoint) = futures_ringbuf::Endpoint::pair(1024, 1024);
    let mut server = Connection::new(server_endpoint, Config::default(), Mode::Server);
    let mut client = Connection::new(client_endpoint, Config::default(), Mode::Client);

    // Pre-register stream on server
    let mut stream_server_side = server.new_stream(stream_id).unwrap();

    // Spawn client, opening a stream, writing to the stream, dropping the stream
    let mut client_stream = client.new_stream(stream_id).unwrap();
    pool.spawner()
        .spawn_obj(
            async move {
                client_stream.write_all(&[42]).await.unwrap();
                drop(client_stream);

                loop {
                    if future::poll_fn(|cx| client.poll(cx)).await.is_ok() {
                        break;
                    }
                }
            }
            .boxed()
            .into(),
        )
        .unwrap();

    // Spawn server connection state machine.
    pool.spawner()
        .spawn_obj(
            async move {
                loop {
                    if future::poll_fn(|cx| server.poll(cx)).await.is_ok() {
                        break;
                    }
                }
            }
            .boxed()
            .into(),
        )
        .unwrap();

    // Expect to eventually receive close on stream.
    pool.run_until(async {
        let mut buf = Vec::new();
        stream_server_side.read_to_end(&mut buf).await?;
        assert_eq!(buf, vec![42]);
        Ok::<(), std::io::Error>(())
    })
    .unwrap();
}

#[test]
fn close_sync() {
    let _ = env_logger::try_init();

    let (server_endpoint, client_endpoint) = futures_ringbuf::Endpoint::pair(1024, 1024);
    let mut config = Config::default();
    config.set_close_sync(true);
    let mut server = Connection::new(server_endpoint, config.clone(), Mode::Server);
    let mut client = Connection::new(client_endpoint, config, Mode::Client);

    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(&waker);

    // Create streams on both sides with same ID
    let stream_id = b"test";
    let client_stream = client.new_stream(stream_id).unwrap();
    let server_stream = server.new_stream(stream_id).unwrap();

    // Write from client (this sends StreamInit + Data)
    assert!(pin!(client_stream).poll_write(&mut cx, b"hello").is_ready());

    // Client initiates close
    client.close();

    // Poll client a bunch of times and ensure it doesn't finish closing yet.
    for _ in 0..10 {
        assert!(client.poll(&mut cx).is_pending());
    }

    // Server polls to receive StreamInit and transition the stream
    let _ = server.poll(&mut cx);
    let _ = server.poll(&mut cx);

    let mut buf = [0u8; 5];
    assert!(pin!(server_stream).poll_read(&mut cx, &mut buf).is_ready());
    assert_eq!(&buf, b"hello");

    // Server polls more to receive GoAway
    while server.poll(&mut cx).is_pending() {}

    // Now server closes
    server.close();
    let _ = server.poll(&mut cx);

    // Client should now be able to finish closing
    while client.poll(&mut cx).is_pending() {}
}
