// Regression: frames queued via the `Handle` path on streams the driver has
// not yet adopted must be flushed by `close()`.
//
// `Handle::new_stream` enqueues the per-stream command Receiver into
// `new_receiver_rx`; the driver only folds it into `stream_receivers` on the
// next `Active::poll` (active.rs). The close paths previously carried only
// `stream_receivers` into `Closing` and dropped `new_receiver_rx` with
// `Active`, so frames written on not-yet-adopted streams were silently lost
// (the write still returned Ok). This test opens via the Handle, queues writes
// without polling the driver, then closes, and asserts the peer receives all
// of them.

use std::pin::pin;
use std::task::{Context, Poll, Waker};

use futures::{AsyncRead, AsyncWrite};
use tlsn_mux::{Config, Connection};

fn cfg() -> Config {
    let mut c = Config::default();
    c.set_keep_alive(true);
    c.set_close_sync(true);
    c
}

#[test]
fn handle_queued_frames_survive_close() {
    let (server_endpoint, client_endpoint) = futures_ringbuf::Endpoint::pair(1 << 20, 1 << 20);

    let mut server = Connection::new(server_endpoint, cfg());
    let mut client = Connection::new(client_endpoint, cfg());
    let client_handle = client.handle().expect("handle while active");

    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);

    let mut sstream = server.new_stream(b"s").unwrap();
    // Open the stream via the Handle: its receiver lands in `new_receiver_rx`,
    // not `stream_receivers`, until the driver polls.
    let mut cstream = client_handle.new_stream(b"s").unwrap();

    // Queue frames WITHOUT polling the client driver, so the receiver is still
    // in `new_receiver_rx` when we close.
    let mut queued = 0usize;
    for i in 0..5 {
        match pin!(&mut cstream).poll_write(&mut cx, b"x") {
            Poll::Ready(Ok(1)) => queued += 1,
            other => panic!("write {i} did not queue: {other:?}"),
        }
    }
    assert_eq!(queued, 5);

    client.close();

    let mut client_done = false;
    let mut server_done = false;
    for _ in 0..5000 {
        if client.poll(&mut cx).is_ready() {
            client_done = true;
        }
        if server.poll(&mut cx).is_ready() {
            server_done = true;
        }
        if client_done && server_done {
            break;
        }
    }
    assert!(client_done, "client connection should have completed");
    assert!(server_done, "server connection should have completed");

    drop(server);

    let mut total = 0usize;
    let mut buf = [0u8; 16];
    loop {
        match pin!(&mut sstream).poll_read(&mut cx, &mut buf) {
            Poll::Ready(Ok(0)) => break,
            Poll::Ready(Ok(n)) => total += n,
            Poll::Ready(Err(e)) => panic!("read error: {e}"),
            Poll::Pending => break,
        }
    }

    assert_eq!(
        total, 5,
        "Handle-queued frames must be flushed by close (not dropped with new_receiver_rx)"
    );
}
