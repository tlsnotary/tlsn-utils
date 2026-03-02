use std::{
    net::{IpAddr, Ipv4Addr},
    time::Duration,
};

use futures::{SinkExt, StreamExt};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};
use websocket_relay::Relay;

/// Connect to a WebSocket server with retries.
/// Retries on connection refused errors, useful for waiting until server is
/// ready.
async fn connect_with_retry(url: &str) -> WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>> {
    for _ in 0..50 {
        match connect_async(url).await {
            Ok((ws, _)) => return ws,
            Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
        }
    }
    panic!("failed to connect to {} after retries", url);
}

/// Test that the relay can proxy between a WebSocket client and a TCP server.
#[tokio::test]
async fn test_tcp_relay() {
    // Start a simple TCP echo server
    let tcp_server = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let tcp_addr = tcp_server.local_addr().unwrap();

    tokio::spawn(async move {
        let (mut socket, _) = tcp_server.accept().await.unwrap();
        let mut buf = [0u8; 1024];
        loop {
            let n = socket.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            socket.write_all(&buf[..n]).await.unwrap();
        }
    });

    // Start the relay
    let relay_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_addr = relay_listener.local_addr().unwrap();

    tokio::spawn(async move {
        Relay::default().run(relay_listener).await.unwrap();
    });

    // Connect to the relay via WebSocket (retries until ready)
    let url = format!("ws://{}/tcp?addr={}", relay_addr, tcp_addr);
    let mut ws = connect_with_retry(&url).await;

    // Send data through the relay
    let test_data = b"hello world";
    ws.send(Message::Binary(test_data.to_vec())).await.unwrap();

    // Receive echoed data
    let msg = ws.next().await.unwrap().unwrap();
    assert_eq!(msg, Message::Binary(test_data.to_vec()));

    // Close the connection
    ws.close(None).await.unwrap();
}

/// Test that the relay can proxy between two WebSocket clients.
#[tokio::test]
async fn test_ws_relay() {
    // Start the relay
    let relay_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_addr = relay_listener.local_addr().unwrap();

    tokio::spawn(async move {
        Relay::default().run(relay_listener).await.unwrap();
    });

    // Connect two clients with the same ID (first connect retries until ready)
    let url = format!("ws://{}/ws?id=test123", relay_addr);
    let mut ws1 = connect_with_retry(&url).await;
    let mut ws2 = connect_with_retry(&url).await;

    // Send from client 1 to client 2
    let test_data = b"hello from client 1";
    ws1.send(Message::Binary(test_data.to_vec())).await.unwrap();

    let msg = ws2.next().await.unwrap().unwrap();
    assert_eq!(msg, Message::Binary(test_data.to_vec()));

    // Send from client 2 to client 1
    let test_data = b"hello from client 2";
    ws2.send(Message::Binary(test_data.to_vec())).await.unwrap();

    let msg = ws1.next().await.unwrap().unwrap();
    assert_eq!(msg, Message::Binary(test_data.to_vec()));

    // Close connections
    ws1.close(None).await.unwrap();
    ws2.close(None).await.unwrap();
}

/// Test that the relay builder API works with local_address.
/// This test verifies the API compiles and runs without error.
/// Actual source IP verification would require network namespaces.
#[tokio::test]
async fn test_tcp_relay_with_local_address() {
    // Start a simple TCP server that accepts one connection
    let tcp_server = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let tcp_addr = tcp_server.local_addr().unwrap();

    tokio::spawn(async move {
        let (mut socket, _) = tcp_server.accept().await.unwrap();
        let mut buf = [0u8; 1024];
        let n = socket.read(&mut buf).await.unwrap();
        socket.write_all(&buf[..n]).await.unwrap();
    });

    // Start the relay with local_address set to loopback
    let relay_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_addr = relay_listener.local_addr().unwrap();

    let relay = Relay::builder()
        .local_address(IpAddr::V4(Ipv4Addr::LOCALHOST))
        .build();

    tokio::spawn(async move {
        relay.run(relay_listener).await.unwrap();
    });

    // Connect and verify it works (retries until ready)
    let url = format!("ws://{}/tcp?addr={}", relay_addr, tcp_addr);
    let mut ws = connect_with_retry(&url).await;

    let test_data = b"test with local_address";
    ws.send(Message::Binary(test_data.to_vec())).await.unwrap();

    let msg = ws.next().await.unwrap().unwrap();
    assert_eq!(msg, Message::Binary(test_data.to_vec()));

    ws.close(None).await.unwrap();
}

/// Test the backwards-compatible run() function.
#[tokio::test]
async fn test_run_function() {
    // Start a simple TCP echo server
    let tcp_server = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let tcp_addr = tcp_server.local_addr().unwrap();

    tokio::spawn(async move {
        let (mut socket, _) = tcp_server.accept().await.unwrap();
        let mut buf = [0u8; 1024];
        let n = socket.read(&mut buf).await.unwrap();
        socket.write_all(&buf[..n]).await.unwrap();
    });

    // Start the relay using the standalone run() function
    let relay_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_addr = relay_listener.local_addr().unwrap();

    tokio::spawn(async move {
        websocket_relay::run(relay_listener).await.unwrap();
    });

    // Connect and verify it works (retries until ready)
    let url = format!("ws://{}/tcp?addr={}", relay_addr, tcp_addr);
    let mut ws = connect_with_retry(&url).await;

    let test_data = b"test run function";
    ws.send(Message::Binary(test_data.to_vec())).await.unwrap();

    let msg = ws.next().await.unwrap().unwrap();
    assert_eq!(msg, Message::Binary(test_data.to_vec()));

    ws.close(None).await.unwrap();
}
