#![doc = include_str!("../README.md")]

#[cfg(all(target_arch = "wasm32", any(target_os = "unknown", target_os = "none")))]
mod wasm;

#[cfg(all(target_arch = "wasm32", any(target_os = "unknown", target_os = "none")))]
pub use wasm::*;

#[cfg(not(all(target_arch = "wasm32", any(target_os = "unknown", target_os = "none"))))]
pub use std::thread::{Builder, JoinHandle, spawn};
