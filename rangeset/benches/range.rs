use criterion::{Criterion, black_box, criterion_group, criterion_main};

use rangeset::{RangeSet, Subset};

pub fn criterion_benchmark(c: &mut Criterion) {
    let mut rangeset_vec = Vec::new();
    for i in 0..10000 {
        if i % 2 == 0 {
            // [0..1, 2..3, ... , 9998..9999].
            rangeset_vec.push(i..i + 1);
        }
    }
    let rangeset = RangeSet::from(rangeset_vec);

    c.bench_function("boundary_range_subset_of_rangeset", |b| {
        b.iter(|| boundary_range_subset_of_rangeset(black_box(&rangeset)))
    });

    c.bench_function("rangeset_subset_of_boundary_rangeset", |b| {
        b.iter(|| rangeset_subset_of_boundary_rangeset(black_box(&rangeset)))
    });
}

// To benchmark the worst case where [range.start] is close to [other.end()],
// i.e. N iterations are needed if there is no boundary short citcuit (N ==
// other.len_ranges()).
fn boundary_range_subset_of_rangeset(other: &RangeSet<u32>) {
    let range = 9997..10005;
    let _ = range.is_subset(other);
}

// To benchmark the worst case where [rangeset.last().start] is close to
// [other.end()], i.e. N iterations of [is_subset()] check are needed if there
// is no boundary short citcuit (N ~= rangeset.len_ranges()).
#[allow(clippy::single_range_in_vec_init)]
fn rangeset_subset_of_boundary_rangeset(rangeset: &RangeSet<u32>) {
    let other = RangeSet::from(vec![0..9998]);
    let _ = rangeset.is_subset(&other);
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
