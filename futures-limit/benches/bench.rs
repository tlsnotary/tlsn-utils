use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use futures::{AsyncReadExt, AsyncWriteExt};
use futures_limit::AsyncWriteLimitExt;
use futures_plex::simplex;
use pollster::FutureExt as _;

const M: usize = 1 << 20;

pub fn criterion_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("rate");

    group.throughput(Throughput::Bytes(M as u64));
    group.bench_function("max", |b| {
        let (mut rx, tx) = simplex(M);
        let mut tx = tx.limit_rate(8 * M, usize::MAX);
        let tx_buf = vec![0; M];
        let mut rx_buf = vec![0; M];

        b.iter(|| {
            async {
                futures::try_join!(tx.write_all(&tx_buf), rx.read_exact(&mut rx_buf)).unwrap();
            }
            .block_on();
            black_box(&rx_buf);
        });
    });

    for mega_bits_per_sec in [10, 100, 1000] {
        // 1 ms of data.
        let size = mega_bits_per_sec * M / 1000 / 8;

        group.throughput(Throughput::Bytes(size as u64));
        group.bench_function(BenchmarkId::from_parameter(mega_bits_per_sec), |b| {
            let (mut rx, tx) = simplex(M);

            // 2ms burst
            let burst = mega_bits_per_sec * M / 500;

            let mut tx = tx.limit_rate(burst, mega_bits_per_sec * M);

            let tx_buf = vec![0; size];
            let mut rx_buf = vec![0; size];

            b.iter(|| {
                async {
                    futures::try_join!(tx.write_all(&tx_buf), rx.read_exact(&mut rx_buf)).unwrap();
                }
                .block_on();
                black_box(&rx_buf);
            });
        });
    }
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
