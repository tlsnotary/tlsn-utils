use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use anyhow::{anyhow, Result};
use futures::{SinkExt, StreamExt as _};
use once_cell::sync::Lazy;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tokio_tungstenite::{
    accept_hdr_async,
    tungstenite::{http::Request, Message},
    WebSocketStream,
};
use tracing::{debug, info, instrument};

#[derive(Debug, Default)]
struct State {
    waiting: HashMap<ConnectionId, WebSocketStream<TcpStream>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ConnectionId(String);

static STATE: Lazy<Arc<Mutex<State>>> = Lazy::new(Default::default);

enum Mode {
    /// Acts a proxy between two websocket clients.
    Ws {
        id: ConnectionId,
        ws: WebSocketStream<TcpStream>,
    },
    /// Acts as a proxy between a websocket client and a TCP server.
    Tcp {
        addr: String,
        ws: WebSocketStream<TcpStream>,
    },
}

/// Runs the websocket relay server with the given TCP listener.
#[instrument]
pub async fn run(listener: TcpListener) -> Result<()> {
    loop {
        let (socket, addr) = listener.accept().await?;
        info!("accepted connection from: {}", addr);

        tokio::spawn(handle_connection(addr, socket));
    }
}

#[instrument(skip(io), err)]
async fn handle_connection(addr: SocketAddr, io: TcpStream) -> Result<()> {
    match accept_ws(io).await? {
        Mode::Ws { id, ws } => {
            tokio::spawn(handle_ws(id, ws));
        }
        Mode::Tcp { addr, ws } => {
            tokio::spawn(handle_tcp(addr, ws));
        }
    }

    Ok(())
}

#[instrument(level = "debug", skip_all, err)]
async fn accept_ws(io: TcpStream) -> Result<Mode> {
    let mut uri = None;

    let mut ws = accept_hdr_async(io, |req: &Request<()>, res| {
        uri = Some(req.uri().clone());

        Ok(res)
    })
    .await?;

    let uri = uri.expect("uri should be set");
    let query = uri
        .query()
        .ok_or_else(|| anyhow!("query string not provided"))?;
    let mut params = form_urlencoded::parse(query.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect::<HashMap<String, String>>();

    match uri.path() {
        "/tcp" => {
            let addr = params
                .remove("addr")
                .ok_or_else(|| anyhow!("addr query parameter not provided"))?;

            return Ok(Mode::Tcp { addr, ws });
        }
        "/ws" => {
            let id = params
                .remove("id")
                .ok_or_else(|| anyhow!("id query parameter not provided"))?;

            return Ok(Mode::Ws {
                id: ConnectionId(id),
                ws,
            });
        }
        _ => {
            ws.close(None).await?;

            return Err(anyhow!("invalid path: {:?}", uri.path()));
        }
    }
}

/// Relays messages between two websocket clients.
#[instrument(level = "debug", skip(ws), err)]
async fn handle_ws(id: ConnectionId, ws: WebSocketStream<TcpStream>) -> Result<()> {
    let peer = {
        let mut state = STATE.lock().unwrap();
        if let Some(peer) = state.waiting.remove(&id) {
            peer
        } else {
            state.waiting.insert(id.clone(), ws);

            debug!("connection waiting");

            return Ok(());
        }
    };

    debug!("started");

    let (left_sink, left_stream) = ws.split();
    let (right_sink, right_stream) = peer.split();

    tokio::try_join!(
        left_stream.forward(right_sink),
        right_stream.forward(left_sink),
    )?;

    debug!("connection closed cleanly");

    Ok(())
}

/// Relays data between a websocket client and a TCP server.
#[instrument(level = "debug", skip(ws), err)]
async fn handle_tcp(addr: String, ws: WebSocketStream<TcpStream>) -> Result<()> {
    let mut tcp = TcpStream::connect(addr).await?;

    let (mut sink, mut stream) = ws.split();
    let (mut rx, mut tx) = tcp.split();

    let fut_tx = async {
        while let Some(msg) = stream.next().await.transpose()? {
            let data = match msg {
                Message::Binary(data) => data,
                Message::Close(_) => {
                    break;
                }
                _ => {
                    return Err(anyhow!("websocket client sent non-binary message"));
                }
            };

            tx.write_all(&data).await?;
        }

        debug!("websocket client closed");

        tx.shutdown().await?;

        Ok(())
    };

    let fut_rx = async {
        // 16KB buffer
        let mut buf = [0; 16 * 1024];
        loop {
            let n = rx.read(&mut buf).await?;

            if n == 0 {
                debug!("tcp server closed");
                sink.close().await?;
                return Ok(());
            }

            sink.send(Message::Binary(buf[..n].to_vec())).await?;
        }
    };

    tokio::try_join!(fut_tx, fut_rx)?;

    Ok(())
}
