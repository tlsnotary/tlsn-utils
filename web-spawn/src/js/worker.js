import init, { web_spawn_start_worker } from "WASM_BINDGEN_SHIM_URL";

self.onmessage = event => {
    const [module_or_path, memory, worker] = event.data;
    init({ module_or_path, memory })
        .catch(err => {
            console.error(err);
            // Propagate to main `onerror`:
            setTimeout(() => {
                throw err;
            });
            throw err;
        })
        .then(() => {
            self.postMessage('ready');

            web_spawn_start_worker(worker);

            close();
        });
};
