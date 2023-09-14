#![no_main]

use std::ops::Range;

use libfuzzer_sys::fuzz_target;

use tlsn_utils_fuzz::assert_invariants;
use utils::range::*;

fn expected_range_union(a: Range<u8>, b: Range<u8>) -> Vec<u8> {
    let mut expected_values = a.chain(b).collect::<Vec<_>>();

    expected_values.sort();
    expected_values.dedup();

    expected_values
}

fuzz_target!(|r: (Range<u8>, Range<u8>)| {
    let (r1, r2) = r;

    let expected_values = expected_range_union(r1.clone(), r2.clone());

    let union = r1.union(&r2);

    let actual_values = union.iter().collect::<Vec<_>>();

    assert_eq!(expected_values, actual_values);

    assert_invariants(union);
});
