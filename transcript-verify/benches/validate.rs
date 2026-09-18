//! Criterion benchmarks: host-side parsing vs guest-side validation vs the
//! `spansy` full-parser baseline, over three representative fixtures
//! (chunked JSON ~26 KB, chunked JSON array ~21 KB, Content-Length JSON
//! ~1 KB).
//!
//! - `host_parse`: [`parse_transcript`] — the untrusted, spansy-backed parse
//!   producing the span table; runs outside the VM, so its cost is off the
//!   proving path.
//! - `guest_validate`: [`validate`] with the table precomputed outside the loop
//!   — the proxy for in-VM cost, and the headline number.
//! - `spansy_parse`: `spansy::http::parse_request` + `parse_response` — the
//!   cost of fully parsing inside the guest instead, i.e. the baseline this
//!   crate exists to beat. (All three fixtures are spansy-parseable; some
//!   corpus shapes — reason-less status lines, close-delimited or HEAD
//!   responses — are not, and are deliberately not benched.)
//!
//! Throughput is measured in transcript bytes (`sent.len() + recv.len()`).

use std::{fs, hint::black_box, path::PathBuf};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use transcript_verify::{parse_transcript, validate};

const FIXTURES: &[&str] = &["pokeapi_ditto", "swapi_films", "httpbingo_get"];

/// Loads a fixture pair as exact wire bytes.
fn load_fixture(name: &str) -> (Vec<u8>, Vec<u8>) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures");
    let read = |ext: &str| {
        let path = dir.join(format!("{name}.{ext}.bin"));
        fs::read(&path).unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()))
    };
    (read("sent"), read("recv"))
}

fn bench_host_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("host_parse");
    group.sample_size(20);
    for &name in FIXTURES {
        let (sent, recv) = load_fixture(name);
        group.throughput(Throughput::Bytes((sent.len() + recv.len()) as u64));
        group.bench_function(BenchmarkId::from_parameter(name), |b| {
            b.iter(|| {
                parse_transcript(black_box(&sent), black_box(&recv)).expect("fixture parses")
            });
        });
    }
    group.finish();
}

fn bench_guest_validate(c: &mut Criterion) {
    let mut group = c.benchmark_group("guest_validate");
    group.sample_size(20);
    for &name in FIXTURES {
        let (sent, recv) = load_fixture(name);
        // The table is the advice computed once on the host; only the in-VM
        // validation walk (and its de-chunk allocation) is measured.
        let table = parse_transcript(&sent, &recv).expect("fixture parses");
        group.throughput(Throughput::Bytes((sent.len() + recv.len()) as u64));
        group.bench_function(BenchmarkId::from_parameter(name), |b| {
            b.iter(|| {
                validate(black_box(&sent), black_box(&recv), black_box(&table))
                    .expect("fixture validates")
            });
        });
    }
    group.finish();
}

fn bench_spansy_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("spansy_parse");
    group.sample_size(20);
    for &name in FIXTURES {
        let (sent, recv) = load_fixture(name);
        group.throughput(Throughput::Bytes((sent.len() + recv.len()) as u64));
        group.bench_function(BenchmarkId::from_parameter(name), |b| {
            b.iter(|| {
                let request = spansy::http::parse_request(black_box(&sent[..]))
                    .expect("spansy parses the request");
                let response = spansy::http::parse_response(black_box(&recv[..]))
                    .expect("spansy parses the response");
                (request, response)
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_host_parse,
    bench_guest_validate,
    bench_spansy_parse
);
criterion_main!(benches);
