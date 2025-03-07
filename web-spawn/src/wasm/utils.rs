#[cfg(feature = "no-bundler")]
pub(crate) fn get_url() -> js_sys::JsString {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen(thread_local_v2, js_namespace = ["import", "meta"], js_name = url)]
        static URL: js_sys::JsString;
    }

    URL.with(Clone::clone)
}
