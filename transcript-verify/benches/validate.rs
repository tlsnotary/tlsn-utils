//! Criterion benchmark: `validate()` vs spansy parse throughput.
//!
//! TODO(P3/T3): replace the placeholder with real `validate()`-vs-spansy
//! comparisons over the fixture corpus once the validator is implemented.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};

fn criterion_benchmark(c: &mut Criterion) {
    // Placeholder so the bench target compiles during scaffolding.
    c.bench_function("scaffold_placeholder", |b| b.iter(|| black_box(42u64)));
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
