use js_sys::Promise;
use wasm_bindgen::prelude::*;
use web_sys::{Blob, MessageEvent, Url, Worker};

/// Returns the URL for the wasm bindgen shim.
pub(crate) fn get_shim_url() -> String {
    js_sys::eval(include_str!("../js/script_path.js"))
        .unwrap_throw()
        .as_string()
        .unwrap_throw()
}

/// Generates worker script as URL encoded blob
pub(crate) fn encode_script(wasm_bindgen_shim_url: &str, template: &str) -> String {
    let script = template.replace("WASM_BINDGEN_SHIM_URL", &wasm_bindgen_shim_url);

    // Create url encoded blob
    let arr = js_sys::Array::new();
    arr.set(0, JsValue::from_str(&script));
    let blob = Blob::new_with_str_sequence(&arr).unwrap();
    let url = Url::create_object_url_with_blob(
        &blob
            .slice_with_f64_and_f64_and_content_type(0.0, blob.size(), "text/javascript")
            .unwrap(),
    )
    .unwrap();

    url
}

pub(crate) fn callback(worker: &Worker) -> Promise {
    Promise::new(&mut |resolve, _reject| {
        // Create a one-time closure that resolves the promise when a message is
        // received.
        let callback = Closure::once(move |event: MessageEvent| {
            // Resolve the promise with the event's data.
            resolve.call1(&JsValue::NULL, &event.data()).unwrap();
        });

        // Attach the callback to the worker's onmessage event.
        worker.set_onmessage(Some(callback.as_ref().unchecked_ref()));

        // Ensure the callback isn't dropped prematurely.
        callback.forget();
    })
}
