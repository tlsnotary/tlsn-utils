# serio

This crate provides the `Sink` and `Stream` traits which are similar to `futures::Sink` and `futures::Stream` except they work with *any* serializable types.

Much of the functionality from this crate was either adapted or directly copied from the [`futures`](https://github.com/rust-lang/futures-rs/) project. Inspiration was also taken from the [`tokio-serde`](https://crates.io/crates/tokio-serde) crate.

# License

Licensed under either of Apache License, Version 2.0 or MIT license at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.