#![no_main]

use core::ops::Range;

use libfuzzer_sys::fuzz_target;

use rangeset_fuzz::{SmallSet, assert_invariants};

use rangeset::prelude::*;

fn expected_difference(a: Range<u8>, b: RangeSet<u8>) -> Vec<u8> {
    a.filter(|x| !b.contains(x)).collect::<Vec<_>>()
}

fuzz_target!(|r: (Range<u8>, SmallSet)| {
    let range = r.0;
    let set: RangeSet<u8> = r.1.into();

    let expected_values = expected_difference(range.clone(), set.clone());

    let diff = range.difference(&set).into_set();

    let actual_values = diff.iter().collect::<Vec<_>>();

    assert_eq!(expected_values, actual_values);

    assert_invariants(diff);
});
