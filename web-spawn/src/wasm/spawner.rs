use futures::{
    StreamExt,
    channel::mpsc::{UnboundedReceiver, unbounded},
};
use js_sys::Promise;
use wasm_bindgen::prelude::*;

use crate::wasm::{
    Closure, SENDER,
    thread::Builder,
    utils::{callback, encode_script, get_shim_url},
    worker::WorkerData,
};

/// Global spawner which spawns closures into web workers.
#[wasm_bindgen]
pub struct Spawner {
    shim_url: String,
    worker_url: String,
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

        let shim_url = get_shim_url();
        let worker_url = encode_script(&shim_url, include_str!("../js/worker.js"));

        Self {
            shim_url,
            worker_url,
            receiver,
        }
    }

    /// Spawns the spawner into a dedicated web worker.
    pub fn spawn(self) -> Promise {
        let options = web_sys::WorkerOptions::new();
        options.set_type(web_sys::WorkerType::Module);
        options.set_name("web_spawn_spawner");

        let script_url = encode_script(&self.shim_url, include_str!("../js/spawner.js"));
        let worker = web_sys::Worker::new_with_options(&script_url, &options).unwrap_throw();

        let data = js_sys::Array::new();
        data.push(&wasm_bindgen::module());
        data.push(&wasm_bindgen::memory());
        data.push(&JsValue::from(Box::into_raw(Box::new(self))));

        worker.post_message(&data).unwrap_throw();

        callback(&worker)
    }

    /// Runs the spawner.
    pub async fn run(mut self) {
        // Spawn a new worker for every closure.
        while let Some((builder, f)) = self.receiver.next().await {
            WorkerData::new(f).spawn(builder, &self.worker_url);
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
