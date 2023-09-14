#![no_main]

use std::ops::Range;

use libfuzzer_sys::fuzz_target;

use tlsn_utils_fuzz::{assert_invariants, SmallSet};

use utils::range::*;

fn expected_difference(a: Range<u8>, b: RangeSet<u8>) -> Vec<u8> {
    a.filter(|x| !b.contains(x)).collect::<Vec<_>>()
}

fuzz_target!(|r: (Range<u8>, SmallSet)| {
    let range = r.0;
    let set: RangeSet<u8> = r.1.into();

    let expected_values = expected_difference(range.clone(), set.clone());

    let diff = range.difference(&set);

    let actual_values = diff.iter().collect::<Vec<_>>();

    assert_eq!(expected_values, actual_values);

    assert_invariants(diff);
});
