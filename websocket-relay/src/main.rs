use std::{env, net::IpAddr};

use anyhow::{Context, Result};
use tokio::net::TcpListener;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let port: u16 = env::var("PROXY_PORT")
        .map(|port| port.parse().expect("port should be valid integer"))
        .unwrap_or(8080);
    let addr: IpAddr = env::var("PROXY_IP")
        .map(|addr| addr.parse().expect("should be valid IP address"))
        .unwrap_or(IpAddr::V4("127.0.0.1".parse().unwrap()));

    let listener = TcpListener::bind((addr, port))
        .await
        .context("failed to bind to address")?;

    info!("listening on: {}", listener.local_addr()?);

    websocket_relay::run(listener).await?;

    Ok(())
}
