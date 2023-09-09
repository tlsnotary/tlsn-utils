use std::ops::Range;

use libfuzzer_sys::arbitrary::{Arbitrary, Result, Unstructured};

use utils::range::*;

#[derive(Debug)]
pub struct SmallSet {
    pub ranges: Vec<Range<u8>>,
}

impl From<SmallSet> for RangeSet<u8> {
    fn from(s: SmallSet) -> Self {
        RangeSet::new(&s.ranges)
    }
}

impl<'a> Arbitrary<'a> for SmallSet {
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        // Generates a set of ranges with a maximum of 8 ranges.
        let count = u8::arbitrary(u)? % 8;

        let mut set = RangeSet::default();
        for _ in 0..count {
            let new: Range<u8> = Range::arbitrary(u)?;
            set = set.union(&new);
        }

        Ok(SmallSet {
            ranges: set.into_inner(),
        })
    }
}
