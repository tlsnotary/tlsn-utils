# web-spawn

[![Crates.io](https://img.shields.io/crates/v/web-spawn.svg)](https://crates.io/crates/web-spawn)
[![Docs.rs](https://docs.rs/web-spawn/badge.svg)](https://docs.rs/web-spawn)

This crate provides a `std::thread` shim for WASM builds targeting web browsers.

It borrows from and is heavily inspired by both [`wasm-bindgen-rayon`](https://crates.io/crates/wasm-bindgen-rayon) and [`wasm_thread`](https://crates.io/crates/wasm_thread) but makes a couple different design choices.

Most notably, spawning is explicitly delegated to run in a background task and must be initialized at the start of the program. This task can either run on the main browser thread, or be moved to a dedicated worker to avoid potential interference from other loads.

# Usage

Add `web-spawn` as a dependency in your `Cargo.toml`:

```toml
web-spawn = { version = "0.3" }
```

Then **you must ensure that spawning is initialized**. One way to do this is to start the spawner from within your WASM module:

```rust,ignore
use wasm_bindgen_futures::JsFuture;

JsFuture::from(web_spawn::start_spawner()).await.unwrap();
```

Alternatively, this can be done on the javascript side:

```javascript
import init, { startSpawner } from /* your package */;

await init();

// Runs the spawner on a dedicated web worker.
await startSpawner();
```

Now, in the rest of your Rust code you can use `web_spawn::spawn` anywhere you would otherwise use `std::thread::spawn`:

```rust
use web_spawn::spawn;

assert_eq!(spawn(|| 1 + 1).join().unwrap(), 2);
```