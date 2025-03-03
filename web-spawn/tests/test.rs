#![cfg(target_arch = "wasm32")]

use std::sync::atomic::AtomicBool;

use futures::channel::oneshot;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::*;
use web_spawn::{spawn, start_spawner};

static INIT: AtomicBool = AtomicBool::new(false);

async fn init() {
    // If it is set return immediately.
    if INIT.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }

    JsFuture::from(start_spawner()).await.unwrap();
}

#[wasm_bindgen_test]
async fn test_pass() {
    init().await;

    let (sender, receiver) = oneshot::channel();
    spawn(|| sender.send(42).unwrap());
    let value = receiver.await.unwrap();

    assert_eq!(value, 42);
}

#[wasm_bindgen_test]
async fn test_join() {
    init().await;

    let (sender, receiver) = oneshot::channel();
    // Blocking join only works on spawned threads.
    spawn(|| {
        let handle = spawn(|| 42);
        assert_eq!(handle.join().unwrap(), 42);
        sender.send(()).unwrap();
    });

    receiver.await.unwrap();
}
