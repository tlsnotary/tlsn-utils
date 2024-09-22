//! Backport of [`Vec::extract_if`](https://doc.rust-lang.org/stable/std/vec/struct.Vec.html#method.extract_if) to stable Rust. Renamed to `filter_drain` to avoid naming conflict upon stabilization.
//!
//! See [tracking issue](https://github.com/rust-lang/rust/issues/43244).
//!
//! [`Original`](https://github.com/rust-lang/rust/blob/8ed95d1d9e149b5242316c91b3849c58f8320470/library/alloc/src/vec/extract_if.rs).

use core::{ptr, slice};

/// Extension trait that backports [`Vec::extract_if`](https://doc.rust-lang.org/stable/std/vec/struct.Vec.html#method.extract_if) which is not stable yet.
///
/// See [tracking issue](https://github.com/rust-lang/rust/issues/43244)
///
/// We call this `FilterDrain` to avoid the naming conflict with the standard library.
pub trait FilterDrain<'a, T, F> {
    type Item;
    type Iter: Iterator<Item = Self::Item> + 'a;

    fn filter_drain(&'a mut self, pred: F) -> Self::Iter;
}

impl<'a, T, F> FilterDrain<'a, T, F> for Vec<T>
where
    F: FnMut(&mut T) -> bool + 'a,
    T: 'a,
{
    type Item = T;
    type Iter = FilterDrainIter<'a, T, F>;

    fn filter_drain(&'a mut self, pred: F) -> FilterDrainIter<'a, T, F> {
        let old_len = self.len();

        // Guard against us getting leaked (leak amplification)
        unsafe {
            self.set_len(0);
        }

        FilterDrainIter {
            vec: self,
            idx: 0,
            del: 0,
            old_len,
            pred,
        }
    }
}

/// An iterator which uses a closure to determine if an element should be removed.
#[derive(Debug)]
#[must_use = "iterators are lazy and do nothing unless consumed"]
pub struct FilterDrainIter<'a, T, F>
where
    F: FnMut(&mut T) -> bool,
{
    pub(super) vec: &'a mut Vec<T>,
    /// The index of the item that will be inspected by the next call to `next`.
    pub(super) idx: usize,
    /// The number of items that have been drained (removed) thus far.
    pub(super) del: usize,
    /// The original length of `vec` prior to draining.
    pub(super) old_len: usize,
    /// The filter test predicate.
    pub(super) pred: F,
}

impl<T, F> Iterator for FilterDrainIter<'_, T, F>
where
    F: FnMut(&mut T) -> bool,
{
    type Item = T;

    fn next(&mut self) -> Option<T> {
        unsafe {
            while self.idx < self.old_len {
                let i = self.idx;
                let v = slice::from_raw_parts_mut(self.vec.as_mut_ptr(), self.old_len);
                let drained = (self.pred)(&mut v[i]);
                // Update the index *after* the predicate is called. If the index
                // is updated prior and the predicate panics, the element at this
                // index would be leaked.
                self.idx += 1;
                if drained {
                    self.del += 1;
                    return Some(ptr::read(&v[i]));
                } else if self.del > 0 {
                    let del = self.del;
                    let src: *const T = &v[i];
                    let dst: *mut T = &mut v[i - del];
                    ptr::copy_nonoverlapping(src, dst, 1);
                }
            }
            None
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.old_len - self.idx))
    }
}

impl<T, F> Drop for FilterDrainIter<'_, T, F>
where
    F: FnMut(&mut T) -> bool,
{
    fn drop(&mut self) {
        unsafe {
            if self.idx < self.old_len && self.del > 0 {
                // This is a pretty messed up state, and there isn't really an
                // obviously right thing to do. We don't want to keep trying
                // to execute `pred`, so we just backshift all the unprocessed
                // elements and tell the vec that they still exist. The backshift
                // is required to prevent a double-drop of the last successfully
                // drained item prior to a panic in the predicate.
                let ptr = self.vec.as_mut_ptr();
                let src = ptr.add(self.idx);
                let dst = src.sub(self.del);
                let tail_len = self.old_len - self.idx;
                src.copy_to(dst, tail_len);
            }
            self.vec.set_len(self.old_len - self.del);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_drain_empty() {
        let mut vec: Vec<i32> = vec![];

        {
            let mut iter = vec.filter_drain(|_| true);
            assert_eq!(iter.size_hint(), (0, Some(0)));
            assert_eq!(iter.next(), None);
            assert_eq!(iter.size_hint(), (0, Some(0)));
            assert_eq!(iter.next(), None);
            assert_eq!(iter.size_hint(), (0, Some(0)));
        }
        assert_eq!(vec.len(), 0);
        assert_eq!(vec, vec![]);
    }

    #[test]
    fn filter_drain_zst() {
        let mut vec = vec![(), (), (), (), ()];
        let initial_len = vec.len();
        let mut count = 0;
        {
            let mut iter = vec.filter_drain(|_| true);
            assert_eq!(iter.size_hint(), (0, Some(initial_len)));
            while let Some(_) = iter.next() {
                count += 1;
                assert_eq!(iter.size_hint(), (0, Some(initial_len - count)));
            }
            assert_eq!(iter.size_hint(), (0, Some(0)));
            assert_eq!(iter.next(), None);
            assert_eq!(iter.size_hint(), (0, Some(0)));
        }

        assert_eq!(count, initial_len);
        assert_eq!(vec.len(), 0);
        assert_eq!(vec, vec![]);
    }

    #[test]
    fn filter_drain_false() {
        let mut vec = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];

        let initial_len = vec.len();
        let mut count = 0;
        {
            let mut iter = vec.filter_drain(|_| false);
            assert_eq!(iter.size_hint(), (0, Some(initial_len)));
            for _ in iter.by_ref() {
                count += 1;
            }
            assert_eq!(iter.size_hint(), (0, Some(0)));
            assert_eq!(iter.next(), None);
            assert_eq!(iter.size_hint(), (0, Some(0)));
        }

        assert_eq!(count, 0);
        assert_eq!(vec.len(), initial_len);
        assert_eq!(vec, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    }

    #[test]
    fn filter_drain_true() {
        let mut vec = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];

        let initial_len = vec.len();
        let mut count = 0;
        {
            let mut iter = vec.filter_drain(|_| true);
            assert_eq!(iter.size_hint(), (0, Some(initial_len)));
            while let Some(_) = iter.next() {
                count += 1;
                assert_eq!(iter.size_hint(), (0, Some(initial_len - count)));
            }
            assert_eq!(iter.size_hint(), (0, Some(0)));
            assert_eq!(iter.next(), None);
            assert_eq!(iter.size_hint(), (0, Some(0)));
        }

        assert_eq!(count, initial_len);
        assert_eq!(vec.len(), 0);
        assert_eq!(vec, vec![]);
    }

    #[test]
    fn filter_drain_complex() {
        {
            //                [+xxx++++++xxxxx++++x+x++]
            let mut vec = vec![
                1, 2, 4, 6, 7, 9, 11, 13, 15, 17, 18, 20, 22, 24, 26, 27, 29, 31, 33, 34, 35, 36,
                37, 39,
            ];

            let removed = vec.filter_drain(|x| *x % 2 == 0).collect::<Vec<_>>();
            assert_eq!(removed.len(), 10);
            assert_eq!(removed, vec![2, 4, 6, 18, 20, 22, 24, 26, 34, 36]);

            assert_eq!(vec.len(), 14);
            assert_eq!(
                vec,
                vec![1, 7, 9, 11, 13, 15, 17, 27, 29, 31, 33, 35, 37, 39]
            );
        }

        {
            //                [xxx++++++xxxxx++++x+x++]
            let mut vec = vec![
                2, 4, 6, 7, 9, 11, 13, 15, 17, 18, 20, 22, 24, 26, 27, 29, 31, 33, 34, 35, 36, 37,
                39,
            ];

            let removed = vec.filter_drain(|x| *x % 2 == 0).collect::<Vec<_>>();
            assert_eq!(removed.len(), 10);
            assert_eq!(removed, vec![2, 4, 6, 18, 20, 22, 24, 26, 34, 36]);

            assert_eq!(vec.len(), 13);
            assert_eq!(vec, vec![7, 9, 11, 13, 15, 17, 27, 29, 31, 33, 35, 37, 39]);
        }

        {
            //                [xxx++++++xxxxx++++x+x]
            let mut vec = vec![
                2, 4, 6, 7, 9, 11, 13, 15, 17, 18, 20, 22, 24, 26, 27, 29, 31, 33, 34, 35, 36,
            ];

            let removed = vec.filter_drain(|x| *x % 2 == 0).collect::<Vec<_>>();
            assert_eq!(removed.len(), 10);
            assert_eq!(removed, vec![2, 4, 6, 18, 20, 22, 24, 26, 34, 36]);

            assert_eq!(vec.len(), 11);
            assert_eq!(vec, vec![7, 9, 11, 13, 15, 17, 27, 29, 31, 33, 35]);
        }

        {
            //                [xxxxxxxxxx+++++++++++]
            let mut vec = vec![
                2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 1, 3, 5, 7, 9, 11, 13, 15, 17, 19,
            ];

            let removed = vec.filter_drain(|x| *x % 2 == 0).collect::<Vec<_>>();
            assert_eq!(removed.len(), 10);
            assert_eq!(removed, vec![2, 4, 6, 8, 10, 12, 14, 16, 18, 20]);

            assert_eq!(vec.len(), 10);
            assert_eq!(vec, vec![1, 3, 5, 7, 9, 11, 13, 15, 17, 19]);
        }

        {
            //                [+++++++++++xxxxxxxxxx]
            let mut vec = vec![
                1, 3, 5, 7, 9, 11, 13, 15, 17, 19, 2, 4, 6, 8, 10, 12, 14, 16, 18, 20,
            ];

            let removed = vec.filter_drain(|x| *x % 2 == 0).collect::<Vec<_>>();
            assert_eq!(removed.len(), 10);
            assert_eq!(removed, vec![2, 4, 6, 8, 10, 12, 14, 16, 18, 20]);

            assert_eq!(vec.len(), 10);
            assert_eq!(vec, vec![1, 3, 5, 7, 9, 11, 13, 15, 17, 19]);
        }
    }

    // FIXME: re-enable emscripten once it can unwind again
    #[test]
    #[cfg(not(target_os = "emscripten"))]
    #[cfg_attr(not(panic = "unwind"), ignore = "test requires unwinding support")]
    fn filter_drain_consumed_panic() {
        use std::rc::Rc;
        use std::sync::Mutex;

        struct Check {
            index: usize,
            drop_counts: Rc<Mutex<Vec<usize>>>,
        }

        impl Drop for Check {
            fn drop(&mut self) {
                self.drop_counts.lock().unwrap()[self.index] += 1;
                println!("drop: {}", self.index);
            }
        }

        let check_count = 10;
        let drop_counts = Rc::new(Mutex::new(vec![0_usize; check_count]));
        let mut data: Vec<Check> = (0..check_count)
            .map(|index| Check {
                index,
                drop_counts: Rc::clone(&drop_counts),
            })
            .collect();

        let _ = std::panic::catch_unwind(move || {
            let filter = |c: &mut Check| {
                if c.index == 2 {
                    panic!("panic at index: {}", c.index);
                }
                // Verify that if the filter could panic again on another element
                // that it would not cause a double panic and all elements of the
                // vec would still be dropped exactly once.
                if c.index == 4 {
                    panic!("panic at index: {}", c.index);
                }
                c.index < 6
            };
            let drain = data.filter_drain(filter);

            // NOTE: The FilterDrainIter is explicitly consumed
            drain.for_each(drop);
        });

        let drop_counts = drop_counts.lock().unwrap();
        assert_eq!(check_count, drop_counts.len());

        for (index, count) in drop_counts.iter().cloned().enumerate() {
            assert_eq!(
                1, count,
                "unexpected drop count at index: {} (count: {})",
                index, count
            );
        }
    }

    // FIXME: Re-enable emscripten once it can catch panics
    #[test]
    #[cfg(not(target_os = "emscripten"))]
    #[cfg_attr(not(panic = "unwind"), ignore = "test requires unwinding support")]
    fn filter_drain_unconsumed_panic() {
        use std::rc::Rc;
        use std::sync::Mutex;

        struct Check {
            index: usize,
            drop_counts: Rc<Mutex<Vec<usize>>>,
        }

        impl Drop for Check {
            fn drop(&mut self) {
                self.drop_counts.lock().unwrap()[self.index] += 1;
                println!("drop: {}", self.index);
            }
        }

        let check_count = 10;
        let drop_counts = Rc::new(Mutex::new(vec![0_usize; check_count]));
        let mut data: Vec<Check> = (0..check_count)
            .map(|index| Check {
                index,
                drop_counts: Rc::clone(&drop_counts),
            })
            .collect();

        let _ = std::panic::catch_unwind(move || {
            let filter = |c: &mut Check| {
                if c.index == 2 {
                    panic!("panic at index: {}", c.index);
                }
                // Verify that if the filter could panic again on another element
                // that it would not cause a double panic and all elements of the
                // vec would still be dropped exactly once.
                if c.index == 4 {
                    panic!("panic at index: {}", c.index);
                }
                c.index < 6
            };
            let _drain = data.filter_drain(filter);

            // NOTE: The FilterDrainIter is dropped without being consumed
        });

        let drop_counts = drop_counts.lock().unwrap();
        assert_eq!(check_count, drop_counts.len());

        for (index, count) in drop_counts.iter().cloned().enumerate() {
            assert_eq!(
                1, count,
                "unexpected drop count at index: {} (count: {})",
                index, count
            );
        }
    }

    #[test]
    fn filter_drain_unconsumed() {
        let mut vec = vec![1, 2, 3, 4];
        let drain = vec.filter_drain(|&mut x| x % 2 != 0);
        drop(drain);
        assert_eq!(vec, [1, 2, 3, 4]);
    }
}
