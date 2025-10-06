#![no_main]

use core::ops::Range;

use libfuzzer_sys::fuzz_target;

use rangeset_fuzz::{SmallSet, assert_invariants};

use rangeset::prelude::*;

fn expected_difference(a: RangeSet<u8>, b: Range<u8>) -> Vec<u8> {
    a.iter().filter(|x| !b.contains(x)).collect::<Vec<_>>()
}

fuzz_target!(|r: (SmallSet, Range<u8>)| {
    let (set, range) = r;

    let set = RangeSet::new_from_slice(&set.ranges);

    let expected_values = expected_difference(set.clone(), range.clone());

    let diff = set.difference(&range).into_set();

    let actual_values = diff.iter().collect::<Vec<_>>();

    assert_eq!(expected_values, actual_values);

    assert_invariants(diff);
});
