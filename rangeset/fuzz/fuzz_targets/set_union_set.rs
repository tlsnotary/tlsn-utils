#![no_main]

use libfuzzer_sys::fuzz_target;

use rangeset_fuzz::{SmallSet, assert_invariants};

use rangeset::prelude::*;

fn expected_union(a: RangeSet<u8>, b: RangeSet<u8>) -> Vec<u8> {
    let mut expected_values = a.iter_values().chain(b.iter_values()).collect::<Vec<_>>();

    expected_values.sort();
    expected_values.dedup();

    expected_values
}

fuzz_target!(|r: (SmallSet, SmallSet)| {
    let r1: RangeSet<u8> = r.0.into();
    let r2: RangeSet<u8> = r.1.into();

    let expected_values = expected_union(r1.clone(), r2.clone());

    let union = r2.union(&r1).into_set();

    let actual_values = union.iter_values().collect::<Vec<_>>();

    assert_eq!(expected_values, actual_values);

    let mut union_mut = r2.clone();
    union_mut.union_mut(&r1);

    assert_eq!(union, union_mut);

    assert_invariants(union);
    assert_invariants(union_mut);
});
