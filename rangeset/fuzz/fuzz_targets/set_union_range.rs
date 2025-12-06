#![no_main]

use core::ops::Range;

use libfuzzer_sys::fuzz_target;

use rangeset_fuzz::{SmallSet, assert_invariants};

use rangeset::prelude::*;

fn expected_union(a: RangeSet<u8>, b: Range<u8>) -> Vec<u8> {
    let mut expected_values = a.iter_values().chain(b).collect::<Vec<_>>();

    expected_values.sort();
    expected_values.dedup();

    expected_values
}

fuzz_target!(|r: (Range<u8>, SmallSet)| {
    let r1 = r.0;
    let r2: RangeSet<u8> = r.1.into();

    let expected_values = expected_union(r2.clone(), r1.clone());

    let union = r2.union(&r1).into_set();

    let actual_values = union.iter_values().collect::<Vec<_>>();

    assert_eq!(expected_values, actual_values);

    assert_invariants(union);
});
