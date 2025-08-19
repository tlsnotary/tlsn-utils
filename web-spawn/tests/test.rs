use std::sync::atomic::AtomicBool;

use futures::channel::oneshot;

use web_spawn::spawn;

#[cfg(all(target_arch = "wasm32", any(target_os = "unknown", target_os = "none")))]
use wasm_bindgen_futures::JsFuture;

static INIT: AtomicBool = AtomicBool::new(false);

async fn init() {
    if !INIT.swap(true, std::sync::atomic::Ordering::SeqCst) {
        #[cfg(all(target_arch = "wasm32", any(target_os = "unknown", target_os = "none")))]
        JsFuture::from(web_spawn::start_spawner()).await.unwrap();
    }
}

#[cfg_attr(
    not(all(target_arch = "wasm32", any(target_os = "unknown", target_os = "none"))),
    pollster::test
)]
#[cfg_attr(
    all(target_arch = "wasm32", any(target_os = "unknown", target_os = "none")),
    wasm_bindgen_test::wasm_bindgen_test
)]
async fn test_pass() {
    init().await;

    let (sender, receiver) = oneshot::channel();
    spawn(|| sender.send(42).unwrap());
    let value = receiver.await.unwrap();

    assert_eq!(value, 42);
}

#[cfg_attr(
    not(all(target_arch = "wasm32", any(target_os = "unknown", target_os = "none"))),
    pollster::test
)]
#[cfg_attr(
    all(target_arch = "wasm32", any(target_os = "unknown", target_os = "none")),
    wasm_bindgen_test::wasm_bindgen_test
)]
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
