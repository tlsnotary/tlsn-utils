#![no_main]

use libfuzzer_sys::fuzz_target;

use tlsn_utils_fuzz::{assert_invariants, SmallSet};

use utils::range::*;

fn expected_union(a: RangeSet<u8>, b: RangeSet<u8>) -> Vec<u8> {
    let mut expected_values = a.iter().chain(b.iter()).collect::<Vec<_>>();

    expected_values.sort();
    expected_values.dedup();

    expected_values
}

fuzz_target!(|r: (SmallSet, SmallSet)| {
    let r1: RangeSet<u8> = r.0.into();
    let r2: RangeSet<u8> = r.1.into();

    let expected_values = expected_union(r1.clone(), r2.clone());

    let union = r2.union(&r1);

    let actual_values = union.iter().collect::<Vec<_>>();

    assert_eq!(expected_values, actual_values);

    assert_invariants(union);
});
