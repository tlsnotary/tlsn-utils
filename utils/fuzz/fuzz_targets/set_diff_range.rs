#![no_main]

use std::ops::Range;

use libfuzzer_sys::fuzz_target;

use tlsn_utils_fuzz::{assert_invariants, SmallSet};

use utils::range::*;

fn expected_difference(a: RangeSet<u8>, b: Range<u8>) -> Vec<u8> {
    a.iter().filter(|x| !b.contains(x)).collect::<Vec<_>>()
}

fuzz_target!(|r: (SmallSet, Range<u8>)| {
    let (set, range) = r;

    let set = RangeSet::new(&set.ranges);

    let expected_values = expected_difference(set.clone(), range.clone());

    let diff = set.difference(&range);

    let actual_values = diff.iter().collect::<Vec<_>>();

    assert_eq!(expected_values, actual_values);

    assert_invariants(diff);
});
