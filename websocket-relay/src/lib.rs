use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Result, anyhow};
use futures::{SinkExt, StreamExt as _};
use once_cell::sync::Lazy;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpSocket, TcpStream},
};
use tokio_tungstenite::{
    WebSocketStream, accept_hdr_async,
    tungstenite::{Message, http::Request},
};
use tracing::{debug, info, instrument};

/// WebSocket relay server.
#[derive(Debug, Clone, Default)]
pub struct Relay {
    /// Local address to bind outgoing TCP connections to.
    ///
    /// If set, the relay will bind to this address before connecting to the
    /// target TCP server. This is useful for controlling the source IP address
    /// of outgoing connections.
    ///
    /// If not set, the operating system will choose the source address based on
    /// the routing table.
    local_address: Option<IpAddr>,
}

impl Relay {
    /// Creates a new relay builder.
    pub fn builder() -> RelayBuilder {
        RelayBuilder::default()
    }

    /// Runs the websocket relay server with the given TCP listener.
    #[instrument(skip(self))]
    pub async fn run(&self, listener: TcpListener) -> Result<()> {
        loop {
            let (socket, addr) = listener.accept().await?;
            info!("accepted connection from: {}", addr);

            let relay = self.clone();
            tokio::spawn(async move { relay.handle_connection(addr, socket).await });
        }
    }

    #[instrument(skip(self, io), err)]
    async fn handle_connection(&self, addr: SocketAddr, io: TcpStream) -> Result<()> {
        match accept_ws(io).await? {
            Mode::Ws { id, ws } => {
                tokio::spawn(handle_ws(id, ws));
            }
            Mode::Tcp { addr, ws } => {
                let local_address = self.local_address;
                tokio::spawn(handle_tcp(addr, local_address, ws));
            }
        }

        Ok(())
    }
}

/// Builder for [`Relay`].
#[derive(Debug, Default)]
pub struct RelayBuilder {
    local_address: Option<IpAddr>,
}

impl RelayBuilder {
    /// Sets the local address to bind outgoing TCP connections to.
    ///
    /// This controls the source IP address used when connecting to target TCP
    /// servers. If not set, the operating system will choose the source address
    /// based on the routing table.
    pub fn local_address(mut self, addr: IpAddr) -> Self {
        self.local_address = Some(addr);
        self
    }

    /// Builds the relay.
    pub fn build(self) -> Relay {
        Relay {
            local_address: self.local_address,
        }
    }
}

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
///
/// This is a convenience function that creates a default [`Relay`] and runs it.
/// For more control over the relay configuration, use [`Relay::builder()`].
#[instrument]
pub async fn run(listener: TcpListener) -> Result<()> {
    Relay::default().run(listener).await
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
async fn handle_tcp(
    addr: String,
    local_address: Option<IpAddr>,
    ws: WebSocketStream<TcpStream>,
) -> Result<()> {
    let mut tcp = match local_address {
        Some(local_addr) => {
            let socket = TcpSocket::new_v4()?;
            socket.bind(SocketAddr::new(local_addr, 0))?;
            socket.connect(addr.parse()?).await?
        }
        None => TcpStream::connect(addr).await?,
    };
    tcp.set_nodelay(true)?;

    let (mut sink, mut stream) = ws.split();
    let (mut rx, mut tx) = tcp.split();

    let is_client_closed = AtomicBool::new(false);

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
        is_client_closed.store(true, Ordering::Relaxed);

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

            // Only send to client if it hasn't closed.
            if !is_client_closed.load(Ordering::Relaxed) {
                sink.send(Message::Binary(buf[..n].to_vec())).await?;
            }
        }
    };

    tokio::try_join!(fut_tx, fut_rx)?;

    Ok(())
}
