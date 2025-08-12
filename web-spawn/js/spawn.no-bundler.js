function registerMessageListener(target, type, callback) {
    const listener = async (event) => {
        const message = event.data;
        if (message && message.type === type) {
            await callback(message.data);
        }
    };

    target.addEventListener('message', listener);
}

// Register listener for the start spawner message.
registerMessageListener(self, 'web_spawn_start_spawner', async (data) => {
    const [module, memory, workerUrl, wasmUrl, spawner] = data;
    const wasm = await import(`${wasmUrl}`);

    const exports = wasm.initSync({ module, memory });
    postMessage('web_spawn_spawner_ready');
    await wasm.web_spawn_recover_spawner(spawner).run(workerUrl);

    exports.__wbindgen_thread_destroy();

    URL.revokeObjectURL(workerUrl);

    close();
});

// Register listener for the start worker message.
registerMessageListener(self, 'web_spawn_start_worker', async (data) => {
    const [module, memory, wasmUrl, worker] = data;
    const wasm = await import(`${wasmUrl}`);

    const exports = wasm.initSync({ module, memory });
    wasm.web_spawn_start_worker(worker);

    exports.__wbindgen_thread_destroy();

    close();
});

/// Starts the spawner in a new worker.
export async function startSpawnerWorker(module, memory, spawner) {
    let workerUrl = import.meta.url;
    let scriptBlob = await fetch(`${workerUrl}`).then(r => r.blob());
    workerUrl = URL.createObjectURL(scriptBlob);

    const worker = new Worker(workerUrl, {
        name: 'web-spawn-spawner',
        type: 'module'
    });

    const wasmUrl = spawner.getUrl();
    worker.postMessage({
        type: 'web_spawn_start_spawner',
        data: [module, memory, workerUrl, wasmUrl, spawner.intoRaw()]
    });

    await new Promise(resolve => {
        worker.addEventListener('message', function handler(event) {
            if (event.data === 'web_spawn_spawner_ready') {
                worker.removeEventListener('message', handler);
                resolve();
            }
        });
    });
}
