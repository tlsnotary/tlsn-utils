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
use tlsn_mux::{Config, Connection, ConnectionError};
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
            let mut connection = Connection::new(socket, cfg1);

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
            let mut connection = Connection::new(socket, cfg2);

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

/// `new_stream` no longer fails when the stream limit is reached: handles are
/// created locally without bound, and the limit only manifests as write
/// backpressure once streams become active on the wire.
#[test]
fn new_stream_never_errors_with_many_handles() {
    let _ = env_logger::try_init();

    let (server_endpoint, _client_endpoint) = futures_ringbuf::Endpoint::pair(1024, 1024);
    let mut cfg = Config::default();
    cfg.set_max_num_streams(2);
    let mut conn = Connection::new(server_endpoint, cfg);

    // Far more handles than max_num_streams; every call succeeds.
    for i in 0..50 {
        let id = format!("stream-{}", i);
        assert!(conn.new_stream(id.as_bytes()).is_ok());
    }
}

/// Once `max_num_streams` streams are active on the wire, a further write
/// blocks with `Poll::Pending`, and becomes ready after an active stream is
/// dropped and its RST is flushed to the peer.
#[test]
fn write_gated_pending_at_limit_then_ready_after_slot_frees() {
    use std::pin::pin;

    let _ = env_logger::try_init();

    let (server_endpoint, client_endpoint) = futures_ringbuf::Endpoint::pair(8192, 8192);
    let mut cfg = Config::default();
    cfg.set_max_num_streams(1);
    let mut client = Connection::new(client_endpoint, cfg.clone());
    // A peer to drain frames off the socket so writes can flush.
    let mut server = Connection::new(server_endpoint, cfg);

    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);

    let mut stream_a = client.new_stream(b"a").unwrap();
    let mut stream_b = client.new_stream(b"b").unwrap();

    // A claims the single slot.
    assert!(pin!(&mut stream_a).poll_write(&mut cx, b"x").is_ready());
    let _ = client.poll(&mut cx);

    // B cannot claim a slot: write is pending.
    assert!(pin!(&mut stream_b).poll_write(&mut cx, b"y").is_pending());

    // Drop A; drive both connections so A's RST is flushed and the slot frees.
    drop(stream_a);
    for _ in 0..50 {
        let _ = client.poll(&mut cx);
        let _ = server.poll(&mut cx);
    }

    // B can now claim the freed slot.
    assert!(pin!(&mut stream_b).poll_write(&mut cx, b"y").is_ready());
}

/// A peer cannot make us buffer for more than `max_num_streams` streams: when
/// the client activates more distinct stream ids than the server's limit, the
/// server terminates rather than buffering unbounded peer-driven streams.
#[test]
fn peer_cannot_exceed_max_streams() {
    let _ = env_logger::try_init();

    async fn run_test() -> Result<(), ConnectionError> {
        // The client may open many streams; the server's limit is small.
        let mut client_cfg = Config::default();
        client_cfg.set_max_num_streams(64);
        let mut server_cfg = Config::default();
        server_cfg.set_max_num_streams(4);

        let (mut server, mut client) = connected_peers(server_cfg, client_cfg, None).await?;

        // Open the streams up-front (always succeeds), then drive the client
        // poll loop while writing so frames reach the server.
        let mut streams = Vec::new();
        for i in 0..32 {
            let id = format!("peer-{}", i);
            streams.push(client.new_stream(id.as_bytes())?);
        }

        let client_task = task::spawn(async move {
            let drive = future::poll_fn(|cx| client.poll(cx));
            let write = async move {
                // Activation happens on first write; hold the handles so no
                // RST frees a slot. Ignore write errors once the server dies.
                for (i, mut s) in streams.into_iter().enumerate() {
                    let _ = s.write_all(&[i as u8]).await;
                    std::mem::forget(s);
                }
                future::pending::<()>().await;
            };
            futures::select_biased! {
                _ = drive.fuse() => {}
                _ = write.fuse() => {}
            }
        });

        // The server must terminate once its limit is exceeded rather than
        // hang buffering unbounded peer streams.
        let server_result = tokio::time::timeout(std::time::Duration::from_secs(5), async move {
            future::poll_fn(|cx| server.poll(cx)).await
        })
        .await;

        client_task.abort();

        assert!(
            matches!(server_result, Ok(Err(_))),
            "server should terminate on exceeding its stream limit, got {server_result:?}"
        );
        Ok(())
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run_test())
        .unwrap();
}

/// A never-activated stream emits nothing on close or drop and consumes no
/// slot, so other streams can still be activated at the limit.
#[test]
fn never_activated_close_and_drop_emit_nothing() {
    use std::pin::pin;

    let _ = env_logger::try_init();

    let (_server_endpoint, client_endpoint) = futures_ringbuf::Endpoint::pair(1024, 1024);
    let mut cfg = Config::default();
    cfg.set_max_num_streams(1);
    let mut client = Connection::new(client_endpoint, cfg);

    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);

    // Create and drop a stream without ever writing; it consumed no slot.
    let unused = client.new_stream(b"unused").unwrap();
    drop(unused);

    // Close a never-written stream: returns Ready, sends no close command.
    let mut to_close = client.new_stream(b"to-close").unwrap();
    assert!(pin!(&mut to_close).poll_close(&mut cx).is_ready());
    drop(to_close);

    // Drive the driver; nothing was owed so it stays pending (no work).
    for _ in 0..10 {
        let _ = client.poll(&mut cx);
    }

    // The single slot is still free: a real stream can activate.
    let mut active = client.new_stream(b"active").unwrap();
    assert!(pin!(&mut active).poll_write(&mut cx, b"z").is_ready());
}

/// A local handle created before the peer opens the same stream id adopts the
/// peer-created entry, so inbound data is delivered without loss.
#[test]
fn split_brain_local_then_peer_data_delivers() {
    let _ = env_logger::try_init();
    let mut pool = LocalPool::new();

    let (server_endpoint, client_endpoint) = futures_ringbuf::Endpoint::pair(8192, 8192);
    let mut server = Connection::new(server_endpoint, Config::default());
    let mut client = Connection::new(client_endpoint, Config::default());

    // Server creates a local handle but does not activate it yet.
    let mut server_stream = server.new_stream(b"shared-id").unwrap();
    let mut client_stream = client.new_stream(b"shared-id").unwrap();

    pool.spawner()
        .spawn_obj(
            async move {
                client_stream.write_all(b"hello").await.unwrap();
                client_stream.close().await.unwrap();
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

    pool.run_until(async {
        let mut buf = Vec::new();
        server_stream.read_to_end(&mut buf).await.unwrap();
        assert_eq!(buf, b"hello");
    });
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
    let mut server = Connection::new(server_endpoint, Config::default());
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
    let mut client = Connection::new(client_endpoint, Config::default());
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
    let mut server = Connection::new(server_endpoint, Config::default());
    let mut client = Connection::new(client_endpoint, Config::default());

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
    let mut server = Connection::new(server_endpoint, config.clone());
    let mut client = Connection::new(client_endpoint, config);

    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);

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

/// A writer blocked on the slot limit is woken when a slot frees, with no lost
/// wakeup: dropping the only active stream and flushing its RST wakes the
/// blocked writer's registered waker.
#[test]
fn blocked_writer_is_woken_on_slot_free() {
    use std::{
        pin::pin,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
    };

    let _ = env_logger::try_init();

    struct CountingWake(AtomicUsize);
    impl futures::task::ArcWake for CountingWake {
        fn wake_by_ref(arc_self: &Arc<Self>) {
            arc_self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    let (server_endpoint, client_endpoint) = futures_ringbuf::Endpoint::pair(8192, 8192);
    let mut cfg = Config::default();
    cfg.set_max_num_streams(1);
    let mut client = Connection::new(client_endpoint, cfg.clone());
    let mut server = Connection::new(server_endpoint, cfg);

    let noop = std::task::Waker::noop();
    let mut noop_cx = std::task::Context::from_waker(noop);

    let counter = Arc::new(CountingWake(AtomicUsize::new(0)));
    let waker = futures::task::waker(counter.clone());
    let mut counted_cx = std::task::Context::from_waker(&waker);

    let mut stream_a = client.new_stream(b"a").unwrap();
    let mut stream_b = client.new_stream(b"b").unwrap();

    // A claims the only slot.
    assert!(pin!(&mut stream_a)
        .poll_write(&mut noop_cx, b"x")
        .is_ready());
    let _ = client.poll(&mut noop_cx);

    // B blocks, registering its counting waker.
    assert!(pin!(&mut stream_b)
        .poll_write(&mut counted_cx, b"y")
        .is_pending());
    assert_eq!(counter.0.load(Ordering::SeqCst), 0);

    // Drop A and drive both sides so A's RST flushes and the slot frees.
    drop(stream_a);
    for _ in 0..50 {
        let _ = client.poll(&mut noop_cx);
        let _ = server.poll(&mut noop_cx);
    }

    // The blocked writer's waker must have fired.
    assert!(
        counter.0.load(Ordering::SeqCst) > 0,
        "blocked writer was not woken on slot free"
    );
    assert!(pin!(&mut stream_b)
        .poll_write(&mut counted_cx, b"y")
        .is_ready());
}

/// A writer parked on the slot limit when the connection is closed must be
/// woken (and surface an error) rather than hang forever.
#[test]
fn blocked_writer_is_woken_on_close() {
    use std::{
        pin::pin,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
    };

    let _ = env_logger::try_init();

    struct CountingWake(AtomicUsize);
    impl futures::task::ArcWake for CountingWake {
        fn wake_by_ref(arc_self: &Arc<Self>) {
            arc_self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    let (_server_endpoint, client_endpoint) = futures_ringbuf::Endpoint::pair(8192, 8192);
    let mut cfg = Config::default();
    cfg.set_max_num_streams(1);
    let mut client = Connection::new(client_endpoint, cfg);

    let noop = std::task::Waker::noop();
    let mut noop_cx = std::task::Context::from_waker(noop);

    let counter = Arc::new(CountingWake(AtomicUsize::new(0)));
    let waker = futures::task::waker(counter.clone());
    let mut counted_cx = std::task::Context::from_waker(&waker);

    let mut stream_a = client.new_stream(b"a").unwrap();
    let mut stream_b = client.new_stream(b"b").unwrap();

    // A claims the only slot; B blocks registering its counting waker.
    assert!(pin!(&mut stream_a)
        .poll_write(&mut noop_cx, b"x")
        .is_ready());
    let _ = client.poll(&mut noop_cx);
    assert!(pin!(&mut stream_b)
        .poll_write(&mut counted_cx, b"y")
        .is_pending());
    assert_eq!(counter.0.load(Ordering::SeqCst), 0);

    // Closing the connection must wake the blocked writer.
    client.close();
    assert!(
        counter.0.load(Ordering::SeqCst) > 0,
        "blocked writer was not woken on close"
    );

    // Drive the close handshake so the stream channels are closed, then the
    // woken writer surfaces an error instead of parking again forever.
    for _ in 0..10 {
        let _ = client.poll(&mut counted_cx);
    }
    assert!(pin!(&mut stream_b)
        .poll_write(&mut counted_cx, b"y")
        .is_ready());
}

/// Reusing a user id after dropping its activated handle reopens a fresh stream
/// at the same slot: the new handle can re-activate (the dropped stream's owed
/// RST is committed to the wire before the slot frees, so it is never dropped)
/// and writes succeed.
#[test]
fn reopen_after_drop_at_limit_succeeds() {
    use std::pin::pin;

    let _ = env_logger::try_init();

    let (server_endpoint, client_endpoint) = futures_ringbuf::Endpoint::pair(8192, 8192);
    let mut cfg = Config::default();
    cfg.set_max_num_streams(1);
    let mut client = Connection::new(client_endpoint, cfg.clone());
    let mut server = Connection::new(server_endpoint, cfg);

    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);

    // Activate X (claims the only slot), then drop it.
    let mut x = client.new_stream(b"x").unwrap();
    assert!(pin!(&mut x).poll_write(&mut cx, b"first").is_ready());
    let _ = client.poll(&mut cx);
    drop(x);

    // Drive both sides so X's RST flushes and the slot frees.
    for _ in 0..50 {
        let _ = client.poll(&mut cx);
        let _ = server.poll(&mut cx);
    }

    // Reopen the same id: a fresh stream re-activates and writes successfully.
    let mut x2 = client.new_stream(b"x").unwrap();
    assert!(pin!(&mut x2).poll_write(&mut cx, b"second").is_ready());
}

/// Two local handles sharing one user id share one `Shared`; dropping one must
/// not reap the active stream while the sibling is alive (which would RST the
/// peer's side and break the survivor). The survivor keeps reading and writing.
#[test]
fn duplicate_handle_not_reaped_until_last_dropped() {
    let _ = env_logger::try_init();
    let mut pool = LocalPool::new();

    let (server_endpoint, client_endpoint) = futures_ringbuf::Endpoint::pair(8192, 8192);
    let mut server = Connection::new(server_endpoint, Config::default());
    let mut client = Connection::new(client_endpoint, Config::default());

    let mut server_stream = server.new_stream(b"dup").unwrap();
    // Two client handles for the same user id share one canonical `Shared`.
    let mut client_a = client.new_stream(b"dup").unwrap();
    let client_b = client.new_stream(b"dup").unwrap();

    // Server connection poll loop, kept alive for the whole test.
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

    // Server echoes whatever it reads back on the same stream.
    pool.spawner()
        .spawn_obj(
            async move {
                let mut buf = [0u8; 3];
                server_stream.read_exact(&mut buf).await.unwrap();
                server_stream.write_all(&buf).await.unwrap();
                server_stream.close().await.unwrap();
                future::pending::<()>().await;
            }
            .boxed()
            .into(),
        )
        .unwrap();

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

    pool.run_until(async {
        // Activate via A, then drop A while B is still alive. If the drop
        // reaped the stream it would RST the peer, and the echo read below
        // would never complete.
        client_a.write_all(b"abc").await.unwrap();
        drop(client_a);

        let mut client_b = client_b;
        let mut buf = [0u8; 3];
        client_b.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"abc");
    });
}

/// A peer opening a brand-new stream is not rejected merely because we hold
/// owed-but-unflushed close frames: the peer's buffering demand is bounded by
/// the number of streams active on the wire, not by close frames we still owe.
/// The server reaches its `max_num_streams` in owed RSTs yet still accepts and
/// delivers data on a fresh peer-opened stream.
#[test]
fn owed_close_does_not_block_peer_stream_creation() {
    let _ = env_logger::try_init();
    let mut pool = LocalPool::new();

    let (server_endpoint, client_endpoint) = futures_ringbuf::Endpoint::pair(8192, 8192);
    let mut server_cfg = Config::default();
    server_cfg.set_max_num_streams(2);
    let mut server = Connection::new(server_endpoint, server_cfg);
    let mut client = Connection::new(client_endpoint, Config::default());

    // Server activates two local-only streams then drops them, leaving up to
    // two owed RSTs (its full slot budget) staged.
    let mut s1 = server.new_stream(b"s1").unwrap();
    let mut s2 = server.new_stream(b"s2").unwrap();
    // Server's handle for the fresh id, adopting the peer-created entry.
    let mut server_fresh = server.new_stream(b"fresh").unwrap();
    let mut client_stream = client.new_stream(b"fresh").unwrap();

    pool.spawner()
        .spawn_obj(
            async move {
                client_stream.write_all(b"hello").await.unwrap();
                client_stream.close().await.unwrap();
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

    pool.spawner()
        .spawn_obj(
            async move {
                s1.write_all(b"a").await.unwrap();
                s2.write_all(b"b").await.unwrap();
                drop(s1);
                drop(s2);
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

    // The server delivers the fresh stream's bytes; if it had terminated on the
    // (incorrect) owed-close gate, this read would never complete.
    pool.run_until(async {
        let mut buf = Vec::new();
        server_fresh.read_to_end(&mut buf).await.unwrap();
        assert_eq!(buf, b"hello");
    });
}

/// Data and EOF from a peer that writes to a stream and drops it (RST) before
/// the local side opens the matching id must be retained: the late local
/// opener reads the data and observes EOF instead of hanging forever.
#[test]
fn peer_writes_and_closes_before_local_open() {
    let _ = env_logger::try_init();

    fn spawn_driver<T>(mut conn: Connection<T>)
    where
        T: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        task::spawn(async move {
            loop {
                if future::poll_fn(|cx| conn.poll(cx)).await.is_ok() {
                    break;
                }
            }
        });
    }

    async fn run_test() {
        let (a, b) = futures_ringbuf::Endpoint::pair(1 << 20, 1 << 20);
        let conn_a = Connection::new(a, Config::default());
        let conn_b = Connection::new(b, Config::default());
        let (ha, hb) = (conn_a.handle().unwrap(), conn_b.handle().unwrap());
        spawn_driver(conn_a);
        spawn_driver(conn_b);

        // Peer B writes to "x" and drops it — fully, before A ever opens "x".
        {
            let mut s = hb.new_stream(b"x").unwrap();
            s.write_all(b"hello").await.unwrap();
            drop(s); // sends DATA("x") then RST("x")
        }

        // Let B's frames be processed on A before A opens the id.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // A opens "x" and reads: the data and EOF must have been retained.
        let mut s = ha.new_stream(b"x").unwrap();
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");
    }

    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(10), run_test())
                .await
                .expect("read hung: peer's data/close was lost before local open");
        });
}

/// Same as above but with a graceful close (FIN) instead of a reset: the late
/// local opener reads the data and observes EOF.
#[test]
fn peer_writes_and_closes_gracefully_before_local_open() {
    let _ = env_logger::try_init();

    fn spawn_driver<T>(mut conn: Connection<T>)
    where
        T: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        task::spawn(async move {
            loop {
                if future::poll_fn(|cx| conn.poll(cx)).await.is_ok() {
                    break;
                }
            }
        });
    }

    async fn run_test() {
        let (a, b) = futures_ringbuf::Endpoint::pair(1 << 20, 1 << 20);
        let conn_a = Connection::new(a, Config::default());
        let conn_b = Connection::new(b, Config::default());
        let (ha, hb) = (conn_a.handle().unwrap(), conn_b.handle().unwrap());
        spawn_driver(conn_a);
        spawn_driver(conn_b);

        {
            let mut s = hb.new_stream(b"x").unwrap();
            s.write_all(b"hello").await.unwrap();
            s.close().await.unwrap(); // sends DATA("x") then FIN("x")
            drop(s);
        }

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let mut s = ha.new_stream(b"x").unwrap();
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");
    }

    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(10), run_test())
                .await
                .expect("read hung: peer's data/close was lost before local open");
        });
}

/// Peer-leads stream churn: the peer completes (writes + drops) many streams
/// before the local side opens any of them. Every completed stream waits in
/// its slot until claimed, so every id the local side later opens must
/// deliver the peer's data and EOF (200 ids stay under the default
/// `max_num_streams` budget).
#[test]
fn peer_leads_stream_churn_data_delivered() {
    let _ = env_logger::try_init();

    const N: usize = 200;

    fn spawn_driver<T>(mut conn: Connection<T>)
    where
        T: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        task::spawn(async move {
            loop {
                if future::poll_fn(|cx| conn.poll(cx)).await.is_ok() {
                    break;
                }
            }
        });
    }

    async fn run_test() {
        let (a, b) = futures_ringbuf::Endpoint::pair(1 << 20, 1 << 20);
        let conn_a = Connection::new(a, Config::default());
        let conn_b = Connection::new(b, Config::default());
        let (ha, hb) = (conn_a.handle().unwrap(), conn_b.handle().unwrap());
        spawn_driver(conn_a);
        spawn_driver(conn_b);

        // B completes every stream before A opens a single one.
        for k in 0..N {
            let id = format!("churn-{k}");
            let mut s = hb.new_stream(id.as_bytes()).unwrap();
            s.write_all(id.as_bytes()).await.unwrap();
            drop(s);
        }

        // A opens each id late and must observe the data and EOF.
        for k in 0..N {
            let id = format!("churn-{k}");
            let mut s = ha.new_stream(id.as_bytes()).unwrap();
            let mut buf = Vec::new();
            s.read_to_end(&mut buf).await.unwrap();
            assert_eq!(buf, id.as_bytes(), "stream {id} lost its data");
        }
    }

    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(15), run_test())
                .await
                .expect("peer-leads churn deadlocked: completed streams were not retained");
        });
}

/// Peer-completed streams that no local handle has claimed sit in their slots
/// — data intact — until they are claimed; claiming and dropping one frees its
/// slot, and a peer exceeding the unclaimed budget terminates the connection
/// loudly rather than having data discarded silently.
#[test]
fn unclaimed_peer_streams_hold_slots_until_claimed() {
    use std::{pin::pin, task::Poll};

    let _ = env_logger::try_init();

    let (server_endpoint, client_endpoint) = futures_ringbuf::Endpoint::pair(8192, 8192);
    let mut server_cfg = Config::default();
    server_cfg.set_max_num_streams(2);
    let mut server = Connection::new(server_endpoint, server_cfg);
    let mut client = Connection::new(client_endpoint, Config::default());

    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);

    /// Open a stream on `client`, write its id and drop it (RST), then drive
    /// both connections so the frames are fully processed.
    fn complete_stream<T: AsyncRead + AsyncWrite + Unpin>(
        id: &[u8],
        client: &mut Connection<T>,
        server: &mut Connection<T>,
        cx: &mut std::task::Context<'_>,
    ) {
        let mut s = client.new_stream(id).unwrap();
        assert!(pin!(&mut s).poll_write(cx, id).is_ready());
        drop(s);
        for _ in 0..50 {
            let _ = client.poll(cx);
            let _ = server.poll(cx);
        }
    }

    // The client completes two streams the server has not claimed yet; they
    // fill the server's entire slot budget and wait there.
    complete_stream(b"a", &mut client, &mut server, &mut cx);
    complete_stream(b"b", &mut client, &mut server, &mut cx);
    assert!(server.poll(&mut cx).is_pending());

    // Claiming one and dropping it frees its slot; the data was retained.
    {
        let mut s = server.new_stream(b"a").unwrap();
        let mut buf = [0u8; 16];
        match pin!(&mut s).poll_read(&mut cx, &mut buf) {
            Poll::Ready(Ok(n)) => assert_eq!(&buf[..n], b"a"),
            other => panic!("expected retained data, got {other:?}"),
        }
        match pin!(&mut s).poll_read(&mut cx, &mut buf) {
            Poll::Ready(Ok(0)) => {}
            other => panic!("expected EOF, got {other:?}"),
        }
    }
    for _ in 0..50 {
        let _ = server.poll(&mut cx);
        let _ = client.poll(&mut cx);
    }

    // The freed slot admits another peer stream...
    complete_stream(b"c", &mut client, &mut server, &mut cx);
    assert!(server.poll(&mut cx).is_pending());

    // ...but exceeding the unclaimed budget terminates the connection: data
    // is never silently discarded to make room.
    complete_stream(b"d", &mut client, &mut server, &mut cx);
    assert!(matches!(server.poll(&mut cx), Poll::Ready(_)));
}

/// A reader parked on a never-activated stream must be woken with EOF when the
/// connection is dropped, instead of hanging forever.
#[test]
fn parked_reader_on_inactive_stream_woken_on_drop() {
    use std::{
        pin::pin,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        task::Poll,
    };

    let _ = env_logger::try_init();

    struct CountingWake(AtomicUsize);
    impl futures::task::ArcWake for CountingWake {
        fn wake_by_ref(arc_self: &Arc<Self>) {
            arc_self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    let (_server_endpoint, client_endpoint) = futures_ringbuf::Endpoint::pair(8192, 8192);
    let client = Connection::new(client_endpoint, Config::default());

    let counter = Arc::new(CountingWake(AtomicUsize::new(0)));
    let waker = futures::task::waker(counter.clone());
    let mut cx = std::task::Context::from_waker(&waker);

    // Read a stream that was never written: it is inactive and the peer never
    // sends to it, so the read parks.
    let mut stream = client
        .handle()
        .unwrap()
        .new_stream(b"never-written")
        .unwrap();
    let mut buf = [0u8; 8];
    assert!(pin!(&mut stream).poll_read(&mut cx, &mut buf).is_pending());
    assert_eq!(counter.0.load(Ordering::SeqCst), 0);

    // Dropping the connection must wake the parked reader with EOF.
    drop(client);
    assert!(
        counter.0.load(Ordering::SeqCst) > 0,
        "parked reader was not woken on connection drop"
    );
    assert!(matches!(
        pin!(&mut stream).poll_read(&mut cx, &mut buf),
        Poll::Ready(Ok(0))
    ));
}

/// A reader parked on a never-activated stream must be woken with EOF when the
/// connection is gracefully closed, instead of hanging forever.
#[test]
fn parked_reader_on_inactive_stream_woken_on_close() {
    use std::{
        pin::pin,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        task::Poll,
    };

    let _ = env_logger::try_init();

    struct CountingWake(AtomicUsize);
    impl futures::task::ArcWake for CountingWake {
        fn wake_by_ref(arc_self: &Arc<Self>) {
            arc_self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    let (_server_endpoint, client_endpoint) = futures_ringbuf::Endpoint::pair(8192, 8192);
    let mut client = Connection::new(client_endpoint, Config::default());

    let counter = Arc::new(CountingWake(AtomicUsize::new(0)));
    let waker = futures::task::waker(counter.clone());
    let mut cx = std::task::Context::from_waker(&waker);

    let mut stream = client.new_stream(b"never-written").unwrap();
    let mut buf = [0u8; 8];
    assert!(pin!(&mut stream).poll_read(&mut cx, &mut buf).is_pending());
    assert_eq!(counter.0.load(Ordering::SeqCst), 0);

    // Initiating a graceful close must wake the parked reader with EOF.
    client.close();
    assert!(
        counter.0.load(Ordering::SeqCst) > 0,
        "parked reader was not woken on connection close"
    );
    assert!(matches!(
        pin!(&mut stream).poll_read(&mut cx, &mut buf),
        Poll::Ready(Ok(0))
    ));
}

/// A reader parked on an active stream must also be woken with EOF on a
/// graceful close: once the connection leaves the active state nothing is
/// delivered to streams anymore.
#[test]
fn parked_reader_on_active_stream_woken_on_close() {
    use std::{
        pin::pin,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        task::Poll,
    };

    let _ = env_logger::try_init();

    struct CountingWake(AtomicUsize);
    impl futures::task::ArcWake for CountingWake {
        fn wake_by_ref(arc_self: &Arc<Self>) {
            arc_self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    let (_server_endpoint, client_endpoint) = futures_ringbuf::Endpoint::pair(8192, 8192);
    let mut client = Connection::new(client_endpoint, Config::default());

    let noop = std::task::Waker::noop();
    let mut noop_cx = std::task::Context::from_waker(noop);

    let counter = Arc::new(CountingWake(AtomicUsize::new(0)));
    let waker = futures::task::waker(counter.clone());
    let mut cx = std::task::Context::from_waker(&waker);

    // Activate the stream with a write, then park a read on it.
    let mut stream = client.new_stream(b"active").unwrap();
    assert!(pin!(&mut stream).poll_write(&mut noop_cx, b"x").is_ready());
    let _ = client.poll(&mut noop_cx);

    let mut buf = [0u8; 8];
    assert!(pin!(&mut stream).poll_read(&mut cx, &mut buf).is_pending());
    assert_eq!(counter.0.load(Ordering::SeqCst), 0);

    client.close();
    assert!(
        counter.0.load(Ordering::SeqCst) > 0,
        "parked reader was not woken on connection close"
    );
    assert!(matches!(
        pin!(&mut stream).poll_read(&mut cx, &mut buf),
        Poll::Ready(Ok(0))
    ));
}

/// A writer parked on the slot limit when the connection errors out must be
/// woken and surface an error on its next poll — not re-park on a slot that
/// will never free.
///
/// Note: the precise multi-threaded race this guards (a woken writer
/// re-polling between `drop_all_streams`'s slot wake and the receivers being
/// closed, then re-parking forever) cannot be reproduced deterministically
/// through the public API; it is prevented structurally by `drop_all_streams`
/// closing the receivers before waking slot waiters. This test pins the
/// observable contract: the parked writer is woken and errors out.
#[test]
fn blocked_writer_is_woken_on_connection_error() {
    use std::{
        pin::pin,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
    };

    let _ = env_logger::try_init();

    struct CountingWake(AtomicUsize);
    impl futures::task::ArcWake for CountingWake {
        fn wake_by_ref(arc_self: &Arc<Self>) {
            arc_self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    let (mut server_endpoint, client_endpoint) = futures_ringbuf::Endpoint::pair(8192, 8192);
    let mut cfg = Config::default();
    cfg.set_max_num_streams(1);
    let mut client = Connection::new(client_endpoint, cfg);

    let noop = std::task::Waker::noop();
    let mut noop_cx = std::task::Context::from_waker(noop);

    let counter = Arc::new(CountingWake(AtomicUsize::new(0)));
    let waker = futures::task::waker(counter.clone());
    let mut counted_cx = std::task::Context::from_waker(&waker);

    // A claims the only slot; B parks on the limit.
    let mut stream_a = client.new_stream(b"a").unwrap();
    let mut stream_b = client.new_stream(b"b").unwrap();
    assert!(pin!(&mut stream_a)
        .poll_write(&mut noop_cx, b"x")
        .is_ready());
    let _ = client.poll(&mut noop_cx);
    assert!(pin!(&mut stream_b)
        .poll_write(&mut counted_cx, b"y")
        .is_pending());
    assert_eq!(counter.0.load(Ordering::SeqCst), 0);

    // Feed garbage into the socket so the connection errors into cleanup.
    assert!(pin!(&mut server_endpoint)
        .poll_write(&mut noop_cx, &[0xFF; 64])
        .is_ready());
    assert!(matches!(
        client.poll(&mut noop_cx),
        std::task::Poll::Ready(Err(_))
    ));

    // The parked writer must have been woken, and must observe the closed
    // channel (an error) instead of re-parking forever or writing into the
    // void.
    assert!(
        counter.0.load(Ordering::SeqCst) > 0,
        "blocked writer was not woken on connection error"
    );
    assert!(matches!(
        pin!(&mut stream_b).poll_write(&mut counted_cx, b"y"),
        std::task::Poll::Ready(Err(_))
    ));
}

/// High-load stress test for the write-gated stream limit.
///
/// Drives far more concurrent streams than the client's slot limit through a
/// real multi-threaded runtime, so the great majority of writes must park on
/// `poll_write` (`Poll::Pending`) and be woken when a slot frees (after the
/// previous stream's RST is flushed). A lost waker or a deadlock would leave
/// some stream tasks parked forever; the whole run is wrapped in a timeout so
/// that surfaces as a failure rather than a hang. Each exchange also verifies
/// its echo, so the data plane is checked under contention.
#[test]
fn high_load_write_gating_no_lost_wakers_or_deadlock() {
    let _ = env_logger::try_init();

    // Far more streams than the client's limit -> heavy slot churn.
    const TOTAL_STREAMS: usize = 256;
    const CLIENT_MAX_STREAMS: usize = 8;
    // Two frames per stream (split_send_size is 16 KiB), kept under the
    // 128 KiB window-update threshold so no window updates (and thus no late
    // frames for an already-reaped stream) are emitted.
    const PAYLOAD: usize = 32 * 1024;

    async fn run_test() {
        // The client is tightly slot-limited to force backpressure. The server
        // has ample slots so it can echo every stream without ever reaching its
        // own limit (there are only TOTAL_STREAMS distinct ids).
        let mut client_cfg = Config::default();
        client_cfg.set_max_num_streams(CLIENT_MAX_STREAMS);
        let mut server_cfg = Config::default();
        server_cfg.set_max_num_streams(TOTAL_STREAMS + 8);

        let (mut server, mut client) = connected_peers(server_cfg, client_cfg, None)
            .await
            .expect("connect peers");

        // Pre-register a handle per id on both sides so the ids merge.
        let mut server_streams = Vec::with_capacity(TOTAL_STREAMS);
        let mut client_streams = Vec::with_capacity(TOTAL_STREAMS);
        for i in 0..TOTAL_STREAMS {
            let id = format!("stream-{i}");
            server_streams.push(server.new_stream(id.as_bytes()).unwrap());
            client_streams.push(client.new_stream(id.as_bytes()).unwrap());
        }

        // Drive both connection state machines.
        task::spawn(async move {
            future::poll_fn(|cx| server.poll(cx)).await.ok();
        });
        task::spawn(async move {
            future::poll_fn(|cx| client.poll(cx)).await.ok();
        });

        // Server echoes each stream until the client closes/resets it.
        let server_tasks: Vec<_> = server_streams
            .into_iter()
            .map(|mut stream| {
                task::spawn(async move {
                    {
                        let (mut r, mut w) = AsyncReadExt::split(&mut stream);
                        futures::io::copy(&mut r, &mut w).await.ok();
                    }
                    stream.close().await.ok();
                })
            })
            .collect();

        // Every client stream writes its payload and reads back the echo
        // concurrently. Only CLIENT_MAX_STREAMS can be active at once; the rest
        // block in `poll_write` until a slot frees. Dropping the stream (no
        // graceful close) exercises the owed-RST path: the slot is released
        // only once that RST is flushed.
        let payload = vec![0xABu8; PAYLOAD];
        let client_tasks: Vec<_> = client_streams
            .into_iter()
            .map(|mut stream| {
                let msg = Msg(payload.clone());
                task::spawn(async move {
                    send_recv_message(&mut stream, &msg).await.unwrap();
                    drop(stream);
                })
            })
            .collect();

        for t in client_tasks {
            t.await.expect("client stream task should complete");
        }
        for t in server_tasks {
            t.await.expect("server stream task should complete");
        }
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(15), run_test())
                .await
                .expect("high-load run timed out: a waker was lost or the mux deadlocked");
        });
}

/// High-churn stress test for slot reclamation.
///
/// Many worker tasks repeatedly open a stream, write to it, and drop it,
/// contending for far fewer slots than there are workers. Every drop must
/// reclaim its slot (flushing the owed RST) and wake a blocked writer; if a
/// single reclamation is lost the limited slots are permanently leaked and the
/// workers wedge. Tens of thousands of drops make a reap/wakeup regression
/// overwhelmingly likely to be caught within the timeout.
#[test]
fn high_load_stream_churn_no_slot_leak_or_deadlock() {
    let _ = env_logger::try_init();

    // A single slot makes a leaked reclamation fatal: if any drop fails to free
    // its slot, the one slot is gone and every worker wedges on the next
    // activation, so the bug surfaces as a hang rather than merely slowing down.
    const CLIENT_MAX_STREAMS: usize = 1;
    const WORKERS: usize = 16;
    const PER_WORKER: usize = 8000;

    async fn run_test() {
        let mut client_cfg = Config::default();
        client_cfg.set_max_num_streams(CLIENT_MAX_STREAMS);
        // The server never claims the peer-completed streams, so every one of
        // them sits in a slot — data retained — until the connection ends.
        // Its budget must therefore cover the full churn volume (and the
        // window limit, which is asserted against the slot count, is lifted).
        let mut server_cfg = Config::default();
        server_cfg.set_max_connection_receive_window(None);
        server_cfg.set_max_num_streams(WORKERS * PER_WORKER + 8);

        let (mut server, mut client) = connected_peers(server_cfg, client_cfg, None)
            .await
            .expect("connect peers");
        let handle = client.handle().expect("client handle");

        task::spawn(async move {
            future::poll_fn(|cx| server.poll(cx)).await.ok();
        });
        task::spawn(async move {
            future::poll_fn(|cx| client.poll(cx)).await.ok();
        });

        // Many workers contend for the single slot, each opening/writing/
        // dropping a fresh stream thousands of times. Every drop must reclaim
        // the slot and wake the next waiter.
        let mut workers = Vec::with_capacity(WORKERS);
        for w in 0..WORKERS {
            let handle = handle.clone();
            workers.push(task::spawn(async move {
                for k in 0..PER_WORKER {
                    // Unique id per stream so every open is a fresh activation.
                    let id = format!("w{w}-{k}");
                    let mut stream = handle.new_stream(id.as_bytes()).unwrap();
                    // First write activates the stream (blocking until the slot
                    // is free); dropping it then owes an RST and frees the slot.
                    stream.write_all(&[0xAB]).await.unwrap();
                    drop(stream);
                }
            }));
        }

        for w in workers {
            w.await.expect("worker should complete all iterations");
        }
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(15), run_test())
                .await
                .expect(
                    "stream churn timed out: a slot was leaked (reap race) or the mux deadlocked",
                );
        });
}
