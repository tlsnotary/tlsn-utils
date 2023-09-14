#![no_main]

use libfuzzer_sys::fuzz_target;

use tlsn_utils_fuzz::{assert_invariants, SmallSet};

use utils::range::*;

fn expected_difference(a: RangeSet<u8>, b: RangeSet<u8>) -> Vec<u8> {
    a.iter().filter(|x| !b.contains(x)).collect::<Vec<_>>()
}

fuzz_target!(|r: (SmallSet, SmallSet)| {
    let s1: RangeSet<u8> = r.0.into();
    let s2: RangeSet<u8> = r.1.into();

    let expected_values = expected_difference(s1.clone(), s2.clone());

    let diff = s1.difference(&s2);

    let actual_values = diff.iter().collect::<Vec<_>>();

    assert_eq!(expected_values, actual_values);

    assert_invariants(diff);
});
