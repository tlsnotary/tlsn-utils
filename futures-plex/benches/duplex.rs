//! Ping-pong benchmark for [`futures_plex::duplex`].
//!
//! One iteration is a full round trip: write `payload` bytes into one end and
//! read them from the other. That is the pattern the mux drives per frame.
//!
//! To compare two revisions, save a baseline on the older one and report the
//! change on the newer one:
//!
//! ```text
//! git worktree add /tmp/before <rev>
//! (cd /tmp/before && cargo bench -p futures-plex --bench duplex -- --save-baseline before)
//! cargo bench -p futures-plex --bench duplex -- --baseline before
//! ```
//!
//! Results are sensitive to heap layout: moving the 4 KiB duplex buffer by a
//! few dozen bytes shifted the 1 KiB case by about 6 ns when this benchmark
//! was written, so only compare numbers produced by the same binary.

use std::{hint::black_box, time::Instant};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use futures::{AsyncReadExt, AsyncWriteExt};

fn ping_pong(c: &mut Criterion) {
    let mut group = c.benchmark_group("duplex_ping_pong");

    for payload in [1usize, 64, 1024] {
        // Created outside `block_on`: a `Drop` that closes the peer may need
        // to run its own executor call, which cannot be nested inside one.
        let (mut client, mut server) = futures_plex::duplex(4096);
        let out = vec![7u8; payload];
        let mut inp = vec![0u8; payload];

        group.throughput(Throughput::Bytes(payload as u64));
        group.bench_function(BenchmarkId::from_parameter(payload), |b| {
            b.iter_custom(|iters| {
                let start = Instant::now();
                futures::executor::block_on(async {
                    for _ in 0..iters {
                        client.write_all(&out).await.unwrap();
                        server.read_exact(&mut inp).await.unwrap();
                    }
                });
                black_box(&inp);
                start.elapsed()
            });
        });
    }

    group.finish();
}

criterion_group!(benches, ping_pong);
criterion_main!(benches);
