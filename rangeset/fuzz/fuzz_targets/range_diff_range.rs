#![no_main]

use core::ops::Range;

use libfuzzer_sys::fuzz_target;

use rangeset::prelude::*;

fn expected_range_difference(a: Range<u8>, b: Range<u8>) -> Vec<u8> {
    a.filter(|x| !b.contains(x)).collect::<Vec<_>>()
}

fuzz_target!(|r: (Range<u8>, Range<u8>)| {
    let (r1, r2) = r;

    let expected_values = expected_range_difference(r1.clone(), r2.clone());
    let actual_values = r1.difference(&r2).flatten().collect::<Vec<_>>();

    assert_eq!(expected_values, actual_values);
});
