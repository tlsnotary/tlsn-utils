use wasm_bindgen::prelude::*;

use crate::wasm::{Closure, thread::Builder};

#[wasm_bindgen]
pub struct WorkerData {
    f: Box<Closure>,
}

impl WorkerData {
    pub(crate) fn new(f: Box<Closure>) -> Self {
        WorkerData { f }
    }

    /// Spawns this worker in a new web worker.
    pub(crate) fn spawn(self, builder: Builder, script_url: &str) {
        let options = web_sys::WorkerOptions::new();
        options.set_type(web_sys::WorkerType::Module);

        if let Some(name) = builder.name {
            options.set_name(&name);
        } else {
            options.set_name("web-spawn-worker");
        }

        let worker = web_sys::Worker::new_with_options(script_url, &options).unwrap_throw();

        let data = js_sys::Array::new();
        data.push(&wasm_bindgen::module());
        data.push(&wasm_bindgen::memory());
        #[cfg(feature = "no-bundler")]
        data.push(&crate::utils::get_url());
        data.push(&JsValue::from(Box::into_raw(Box::new(self))));

        let msg = js_sys::Object::new();
        js_sys::Reflect::set(&msg, &"type".into(), &"web_spawn_start_worker".into()).unwrap_throw();
        js_sys::Reflect::set(&msg, &"data".into(), &data).unwrap_throw();

        worker.post_message(&msg).unwrap_throw();
    }
}

#[wasm_bindgen]
#[doc(hidden)]
pub fn web_spawn_start_worker(worker: *mut WorkerData) {
    // # Safety
    // This is safe because we know the worker was allocated on the heap with
    // `Box`. Afterwhich, it was converted to a raw pointer using
    // `Box::into_raw` which prevents it from being deallocated.
    let WorkerData { f } = unsafe { *Box::from_raw(worker) };

    f();
}
