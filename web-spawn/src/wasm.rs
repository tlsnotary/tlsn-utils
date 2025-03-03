#[cfg(all(target_arch = "wasm32", not(doc), not(target_feature = "atomics")))]
compile_error!("web-spawn requires the `atomics` and `bulk-memory` features to be enabled");

mod spawner;
mod thread;
pub(crate) mod utils;
mod worker;

pub use spawner::Spawner;
pub use thread::Builder;

use std::{any::Any, sync::OnceLock};

use crossbeam_channel::Receiver;
use futures::channel::mpsc::UnboundedSender;
use js_sys::Promise;
use wasm_bindgen::prelude::*;

pub(crate) type Closure = dyn FnOnce() + Send;

/// Global sender channel for spawning threads.
pub(crate) static SENDER: OnceLock<UnboundedSender<(Builder, Box<Closure>)>> = OnceLock::new();

/// Initializes the thread spawner.
#[wasm_bindgen(js_name = initSpawner)]
pub fn init_spawner() -> Spawner {
    Spawner::new()
}

/// Starts the thread spawner on a dedicated worker thread.
#[wasm_bindgen(js_name = startSpawner)]
pub fn start_spawner() -> Promise {
    Spawner::new().spawn()
}

/// Spawns a closure onto a new thread.
pub fn spawn<F, T>(f: F) -> JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    Builder::new()
        .spawn(f)
        .expect("spawner should be initialized")
}

/// A join handle for a spawned thread.
#[derive(Debug)]
pub struct JoinHandle<T>(Receiver<std::thread::Result<T>>);

impl<T> JoinHandle<T> {
    /// Returns `true` if the thread has finished.
    pub fn is_finished(&self) -> bool {
        self.0.is_empty()
    }

    /// Waits for the thread to finish and returns the result.
    pub fn join(self) -> std::thread::Result<T> {
        self.0
            .recv()
            .map_err(|_| Box::new("worker thread dropped return channel") as Box<dyn Any + Send>)?
    }
}
