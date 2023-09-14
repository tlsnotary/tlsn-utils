#![no_main]

use std::ops::Range;

use libfuzzer_sys::fuzz_target;

use tlsn_utils_fuzz::{assert_invariants, SmallSet};

use utils::range::*;

fn expected_union(a: RangeSet<u8>, b: Range<u8>) -> Vec<u8> {
    let mut expected_values = a.iter().chain(b).collect::<Vec<_>>();

    expected_values.sort();
    expected_values.dedup();

    expected_values
}

fuzz_target!(|r: (Range<u8>, SmallSet)| {
    let r1 = r.0;
    let r2: RangeSet<u8> = r.1.into();

    let expected_values = expected_union(r2.clone(), r1.clone());

    let union = r2.union(&r1);

    let actual_values = union.iter().collect::<Vec<_>>();

    assert_eq!(expected_values, actual_values);

    assert_invariants(union);
});
