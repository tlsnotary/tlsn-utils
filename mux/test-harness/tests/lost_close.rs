//! Regression tests for a lost data + close when the peer creates, writes, and
//! closes a stream before the local side opens that id (the "peer-leads"
//! pattern). Before the fix this discarded the buffered data and the EOF, so a
//! later `new_stream` for the same id hung reading a fresh `Open` stream.

use std::time::Duration;

use futures::{AsyncReadExt, AsyncWriteExt};
use tlsn_mux::{Config, Connection, Handle};
use tokio_util::compat::TokioAsyncReadCompatExt;

fn spawn_driver<T>(mut conn: Connection<T>)
where
    T: futures::AsyncRead + futures::AsyncWrite + Send + Unpin + 'static,
{
    tokio::spawn(async move {
        loop {
            if futures::future::poll_fn(|cx| conn.poll(cx)).await.is_ok() {
                break;
            }
        }
    });
}

/// Minimal deterministic case: the peer fully writes and closes "x" before the
/// local app opens it. The local open must still see the data and EOF.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn peer_writes_and_closes_before_local_open() {
    let _ = env_logger::try_init();

    let (a, b) = tokio::io::duplex(1 << 20);
    let conn_a = Connection::new(a.compat(), Config::default());
    let conn_b = Connection::new(b.compat(), Config::default());
    let (ha, hb) = (conn_a.handle().unwrap(), conn_b.handle().unwrap());
    spawn_driver(conn_a);
    spawn_driver(conn_b);

    // Peer B writes to "x" and drops it — fully, before A ever opens "x".
    {
        let mut s = hb.new_stream(b"x").unwrap();
        s.write_all(b"hello").await.unwrap();
        drop(s); // sends DATA("x") then RST("x")
    }

    // Let B's frames be processed on A (A implicitly creates "x", then the
    // reset arrives while A has no handle for it yet).
    tokio::time::sleep(Duration::from_millis(200)).await;

    // A opens "x" and reads. It must observe "hello" then EOF.
    let mut s = ha.new_stream(b"x").unwrap();
    let mut buf = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut buf)).await;

    assert!(read.is_ok(), "read hung: peer's data/close was lost before local open");
    assert_eq!(&buf, b"hello", "data lost: got {buf:?}");
}

/// Paced concurrent churn: many short request/response round-trips on fresh ids
/// with bounded concurrency, both peers opening the same ids (so they merge).
/// Each request awaits its response, so the load is self-paced (no unbounded
/// flooding). Exercises implicit-create + merge + close delivery under churn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_churn_request_response() {
    use futures::stream::StreamExt;

    let _ = env_logger::try_init();

    const N: usize = 6000;
    const CONC: usize = 64;

    let (a, b) = tokio::io::duplex(1 << 22);
    let conn_a = Connection::new(a.compat(), Config::default());
    let conn_b = Connection::new(b.compat(), Config::default());
    let (ha, hb) = (conn_a.handle().unwrap(), conn_b.handle().unwrap());
    spawn_driver(conn_a);
    spawn_driver(conn_b);

    // Requester: open id, send a byte, read the echo, close.
    async fn requester(handle: Handle, n: usize, conc: usize) {
        futures::stream::iter(0..n)
            .for_each_concurrent(conc, |i| {
                let handle = handle.clone();
                async move {
                    let id = (i as u64).to_le_bytes();
                    let mut s = handle.new_stream(&id).unwrap();
                    s.write_all(&[0xAB]).await.unwrap();
                    let mut buf = [0u8; 1];
                    s.read_exact(&mut buf).await.unwrap();
                    assert_eq!(buf[0], 0xAB);
                    drop(s);
                }
            })
            .await;
    }

    // Responder: open the same id, read the byte, echo it, close.
    async fn responder(handle: Handle, n: usize, conc: usize) {
        futures::stream::iter(0..n)
            .for_each_concurrent(conc, |i| {
                let handle = handle.clone();
                async move {
                    let id = (i as u64).to_le_bytes();
                    let mut s = handle.new_stream(&id).unwrap();
                    let mut buf = [0u8; 1];
                    if s.read_exact(&mut buf).await.is_ok() {
                        let _ = s.write_all(&buf).await;
                    }
                    drop(s);
                }
            })
            .await;
    }

    let res = tokio::time::timeout(
        Duration::from_secs(30),
        futures::future::join(requester(ha, N, CONC), responder(hb, N, CONC)),
    )
    .await;

    assert!(res.is_ok(), "churn did not complete within 30s (deadlock/regression)");
}
