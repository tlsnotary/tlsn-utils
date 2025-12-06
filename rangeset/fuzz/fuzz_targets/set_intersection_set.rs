#![no_main]

use std::collections::HashSet;

use libfuzzer_sys::fuzz_target;

use rangeset_fuzz::{SmallSet, assert_invariants};

use rangeset::prelude::*;

fuzz_target!(|r: (SmallSet, SmallSet)| {
    let s1: RangeSet<u8> = r.0.into();
    let s2: RangeSet<u8> = r.1.into();

    let h1: HashSet<u8> = HashSet::from_iter(s1.iter_values());
    let h2: HashSet<u8> = HashSet::from_iter(s2.iter_values());

    let intersection = s1.intersection(&s2).into_set();
    let h3: HashSet<u8> = HashSet::from_iter(intersection.iter_values());

    assert_eq!(h3, h1.intersection(&h2).copied().collect::<HashSet<_>>());

    assert_invariants(intersection);
});
