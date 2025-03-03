import init, { web_spawn_recover_spawner } from "WASM_BINDGEN_SHIM_URL";

console.log('spawner spawned');

self.onmessage = event => {
    const [module_or_path, memory, spawner] = event.data;
    init({ module_or_path, memory })
        .catch(err => {
            console.error(err);
            // Propagate to main `onerror`:
            setTimeout(() => {
                throw err;
            });
            throw err;
        })
        .then(async () => {
            self.postMessage('ready');

            await web_spawn_recover_spawner(spawner).run();

            close();
        });
};
