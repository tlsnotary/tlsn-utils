#!/bin/bash

# Test from main browser thread
WASM_BINDGEN_USE_BROWSER=1 wasm-pack test --headless --chrome --firefox -- --features no-bundler

# Test from worker thread
WASM_BINDGEN_USE_DEDICATED_WORKER=1 wasm-pack test --headless --chrome --firefox -- --features no-bundler
