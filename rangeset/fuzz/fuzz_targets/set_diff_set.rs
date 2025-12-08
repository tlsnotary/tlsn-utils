#![no_main]

use libfuzzer_sys::fuzz_target;

use rangeset_fuzz::{SmallSet, assert_invariants};

use rangeset::prelude::*;

fn expected_difference(a: RangeSet<u8>, b: RangeSet<u8>) -> Vec<u8> {
    a.iter_values()
        .filter(|x| !b.contains(x))
        .collect::<Vec<_>>()
}

fuzz_target!(|r: (SmallSet, SmallSet)| {
    let s1: RangeSet<u8> = r.0.into();
    let s2: RangeSet<u8> = r.1.into();

    let expected_values = expected_difference(s1.clone(), s2.clone());

    let diff = s1.difference(&s2).into_set();

    let actual_values = diff.iter_values().collect::<Vec<_>>();

    assert_eq!(expected_values, actual_values);

    assert_invariants(diff);
});
