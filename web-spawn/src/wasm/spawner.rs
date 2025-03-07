use futures::{
    StreamExt,
    channel::mpsc::{UnboundedReceiver, unbounded},
};
use wasm_bindgen::prelude::*;

use crate::wasm::{Closure, SENDER, thread::Builder, worker::WorkerData};

/// Global spawner which spawns closures into web workers.
#[wasm_bindgen]
pub struct Spawner {
    receiver: UnboundedReceiver<(Builder, Box<Closure>)>,
}

#[wasm_bindgen]
impl Spawner {
    /// Creates a new spawner.
    ///
    /// # Panics
    ///
    /// Panics if the spawner is already initialized.
    pub(crate) fn new() -> Self {
        let (sender, receiver) = unbounded();

        if let Err(_) = SENDER.set(sender) {
            panic!("spawner already initialized");
        }

        Self { receiver }
    }

    #[cfg(feature = "no-bundler")]
    #[wasm_bindgen(js_name = getUrl)]
    pub fn get_url(&self) -> js_sys::JsString {
        crate::utils::get_url()
    }

    #[wasm_bindgen(js_name = intoRaw)]
    pub fn into_raw(self) -> *mut Self {
        Box::into_raw(Box::new(self))
    }

    /// Runs the spawner.
    #[wasm_bindgen]
    pub async fn run(&mut self, url: String) {
        // Spawn a new worker for every closure.
        while let Some((builder, f)) = self.receiver.next().await {
            WorkerData::new(f).spawn(builder, &url);
        }
    }
}

#[wasm_bindgen]
#[doc(hidden)]
pub fn web_spawn_recover_spawner(spawner: *mut Spawner) -> Spawner {
    // # Safety
    // This is safe because we know the spawner was allocated on the heap with
    // `Box`. Afterwhich, it was converted to a raw pointer using
    // `Box::into_raw` which prevents it from being deallocated.
    unsafe { *Box::from_raw(spawner) }
}
