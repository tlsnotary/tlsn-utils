use std::{
    collections::HashMap,
    env,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
};

use anyhow::{anyhow, Context, Result};
use futures::StreamExt as _;
use once_cell::sync::Lazy;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{accept_hdr_async, tungstenite::http::Request, WebSocketStream};
use tracing::{debug, info, instrument};
use tracing_subscriber::EnvFilter;

fn init_global_subscriber() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
}

#[derive(Debug, Default)]
struct State {
    connections: HashMap<ConnectionId, WebSocketStream<TcpStream>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ConnectionId(String);

static STATE: Lazy<Arc<Mutex<State>>> = Lazy::new(|| Default::default());

#[tokio::main]
#[instrument]
async fn main() -> Result<()> {
    init_global_subscriber();

    let port: u16 = env::var("PORT")
        .map(|port| port.parse().expect("port should be valid integer"))
        .unwrap_or(8080);
    let addr: IpAddr = env::var("ADDR")
        .map(|addr| addr.parse().expect("addr should be valid IP address"))
        .unwrap_or(IpAddr::V4("127.0.0.1".parse().unwrap()));

    let listener = TcpListener::bind((addr, port))
        .await
        .context("failed to bind to address")?;

    info!("listening on: {}", listener.local_addr()?);

    loop {
        let (socket, addr) = listener.accept().await?;
        info!("accepted connection from: {}", addr);

        tokio::spawn(handle_connection(addr, socket));
    }
}

#[instrument(skip(io), err)]
async fn handle_connection(addr: SocketAddr, io: TcpStream) -> Result<()> {
    let (id, ws) = accept_ws(io).await?;

    let mut state = STATE.lock().unwrap();
    if let Some(peer_connection) = state.connections.remove(&id) {
        tokio::spawn(relay(id, ws, peer_connection));
    } else {
        state.connections.insert(id, ws);
    }

    Ok(())
}

#[instrument(level = "debug", skip_all, err)]
async fn accept_ws(io: TcpStream) -> Result<(ConnectionId, WebSocketStream<TcpStream>)> {
    let mut query = None;

    let mut ws = accept_hdr_async(io, |req: &Request<()>, res| {
        query = Some(
            req.uri()
                .query()
                .map(ToString::to_string)
                .unwrap_or_default(),
        );

        Ok(res)
    })
    .await?;

    let query = query.ok_or_else(|| anyhow!("no query parameters provided in websocket url"))?;
    let params = form_urlencoded::parse(query.as_bytes());

    for (key, value) in params {
        if key == "id" {
            return Ok((ConnectionId(value.into_owned()), ws));
        }
    }

    ws.close(None).await?;

    Err(anyhow!("id query parameter not provided in websocket url"))
}

#[instrument(level = "debug", skip(left, right), err)]
async fn relay(
    id: ConnectionId,
    left: WebSocketStream<TcpStream>,
    right: WebSocketStream<TcpStream>,
) -> Result<()> {
    debug!("starting relay");

    let (left_sink, left_stream) = left.split();
    let (right_sink, right_stream) = right.split();

    tokio::try_join!(
        left_stream.forward(right_sink),
        right_stream.forward(left_sink),
    )?;

    debug!("connection closed cleanly: {:?}", id);

    Ok(())
}
