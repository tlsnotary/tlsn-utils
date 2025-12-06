#![no_main]

use core::ops::Range;

use libfuzzer_sys::fuzz_target;

use rangeset_fuzz::{SmallSet, assert_invariants};

use rangeset::prelude::*;

fn expected_union(a: Range<u8>, b: RangeSet<u8>) -> Vec<u8> {
    let mut expected_values = a.chain(b.iter_values()).collect::<Vec<_>>();

    expected_values.sort();
    expected_values.dedup();

    expected_values
}

fuzz_target!(|r: (Range<u8>, SmallSet)| {
    let r1 = r.0;
    let r2: RangeSet<u8> = r.1.into();

    let expected_values = expected_union(r1.clone(), r2.clone());

    let union = r1.union(&r2).into_set();

    let actual_values = union.iter_values().collect::<Vec<_>>();

    assert_eq!(expected_values, actual_values);

    assert_invariants(union);
});
