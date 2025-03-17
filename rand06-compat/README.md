# rand06-compat

This crate provides a compatibility wrapper between versions `0.6` and `0.9` of [`rand_core`](https://docs.rs/rand_core/0.9).

# Warning about `no_std` ⚠️

Supporting fallible RNGs is tricky to do in a clean manner. This compat wrapper hardcodes an error code of `1` in the case that the RNG fails.

PRs welcome for a better solution.

# Example

```rust
use rand_core_06::RngCore as RngCore06;
use rand_core::OsRng;
use rand06_compat::Rand0_6CompatExt;

// This function requires an RNG implementing the `0.6` trait.
fn foo<R>(rng: &mut R) where R: RngCore06 {
    let num: u32 = rng.next_u32();
    println!("random number: {}", num);
}

// Pass in the RNG from `0.9` using compat trait.
foo(&mut OsRng.compat())
```