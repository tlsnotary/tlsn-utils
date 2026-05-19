#[cfg(all(target_arch = "wasm32", not(doc), not(target_feature = "atomics")))]
compile_error!("web-spawn requires the `atomics` and `bulk-memory` features to be enabled");

mod spawner;
mod thread;
pub(crate) mod utils;
mod worker;

pub use spawner::Spawner;
pub use thread::Builder;

use std::{any::Any, io, num::NonZero, sync::OnceLock};

use crossbeam_channel::Receiver;
use futures::channel::mpsc::UnboundedSender;
use js_sys::Promise;
use wasm_bindgen::prelude::*;

pub(crate) type Closure = dyn FnOnce() + Send;

/// Global sender channel for spawning threads.
pub(crate) static SENDER: OnceLock<UnboundedSender<(Builder, Box<Closure>)>> = OnceLock::new();

#[cfg_attr(not(feature = "no-bundler"), wasm_bindgen(module = "/js/spawn.js"))]
#[cfg_attr(
    feature = "no-bundler",
    wasm_bindgen(module = "/js/spawn.no-bundler.js")
)]
extern "C" {
    #[wasm_bindgen(js_name = startSpawnerWorker)]
    fn start_spawner_worker(module: JsValue, memory: JsValue, spawner: Spawner) -> Promise;
}

/// Returns an estimate of the default amount of parallelism a program should
/// use.
///
/// Reads `navigator.hardwareConcurrency` from the current global scope.
pub fn available_parallelism() -> io::Result<NonZero<usize>> {
    let global = js_sys::global();
    let concurrency = if let Some(scope) = global.dyn_ref::<web_sys::WorkerGlobalScope>() {
        scope.navigator().hardware_concurrency()
    } else if let Some(window) = global.dyn_ref::<web_sys::Window>() {
        window.navigator().hardware_concurrency()
    } else {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "`navigator` is not available in the current global scope",
        ));
    };
    NonZero::new(concurrency as usize).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "`navigator.hardwareConcurrency` is zero",
        )
    })
}

/// Starts the thread spawner on a dedicated worker thread.
#[wasm_bindgen(js_name = startSpawner)]
pub fn start_spawner() -> Promise {
    start_spawner_worker(
        wasm_bindgen::module(),
        wasm_bindgen::memory(),
        Spawner::new(),
    )
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
