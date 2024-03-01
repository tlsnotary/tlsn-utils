use serde::{Deserialize, Serialize};
use serio::{
    codec::{Bincode, Framed},
    IoSink, IoStream, SinkExt as _, StreamExt as _,
};
use std::io::Result;
use tokio::io::duplex;
use tokio_util::codec::LengthDelimitedCodec;

#[derive(Serialize, Deserialize)]
struct Ping(String);

#[derive(Serialize, Deserialize)]
struct Pong(u32);

#[tokio::main]
async fn main() {
    let (a, b) = duplex(1024);

    let a = LengthDelimitedCodec::builder().new_framed(a);
    let b = LengthDelimitedCodec::builder().new_framed(b);

    let a = Framed::new(a, Bincode::default());
    let b = Framed::new(b, Bincode::default());

    tokio::try_join!(alice(a), bob(b)).unwrap();
}

async fn alice<T: IoSink + IoStream + Unpin>(mut io: T) -> Result<()> {
    io.send(Ping("hello bob!".to_string())).await?;
    println!("alice: sent ping");

    let Pong(num) = io.next().await.unwrap()?;
    println!("alice: received pong {num}");

    Ok(())
}

async fn bob<T: IoSink + IoStream + Unpin>(mut io: T) -> Result<()> {
    let Ping(msg) = io.next().await.unwrap()?;
    println!("bob: received ping \"{msg}\"");

    io.send(Pong(42)).await?;
    println!("bob: sent pong");

    Ok(())
}
