// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The entries of the tasks or interrupt groups, by name, ported from
//! `StatisticsContainer.h` and `StatisticsIterator.h`.

use core::cell::Cell;

use crate::stack::{NestedRuntimeEntry, SimpleRuntimeEntry, StackEntry};
use crate::statistics::Statistics;

/// Names the entry with an index; `None` for an unused slot, which the iterator skips.
pub type GetName = dyn Fn(usize) -> Option<&'static [u8]> + Sync;

/// An entry that carries statistics: what a container copies and the writer formats.
pub trait HasStatistics {
    /// The statistics type.
    type Statistics: Statistics;
    /// The statistics.
    fn statistics(&self) -> &Self::Statistics;
}

impl<S: Statistics> HasStatistics for S {
    type Statistics = S;
    fn statistics(&self) -> &S {
        self
    }
}

impl<S: Statistics, const CUT_OUT: bool> HasStatistics for SimpleRuntimeEntry<S, CUT_OUT> {
    type Statistics = S;
    fn statistics(&self) -> &S {
        &self.statistics
    }
}

impl<S: Statistics, const CUT_OUT: bool, N: StackEntry> HasStatistics
    for NestedRuntimeEntry<S, CUT_OUT, N>
{
    type Statistics = S;
    fn statistics(&self) -> &S {
        &self.statistics
    }
}

/// A `static` array of entries and the function that names them.
pub struct StatisticsContainer<E: 'static> {
    entries: &'static [E],
    get_name: Cell<Option<&'static GetName>>,
}

// SAFETY: `get_name` changes only in `copy_from`, under the caller's lock.
unsafe impl<E: Sync> Sync for StatisticsContainer<E> {}

impl<E: 'static> StatisticsContainer<E> {
    /// A container over `entries`, named by `get_name`.
    pub const fn new(entries: &'static [E], get_name: Option<&'static GetName>) -> Self {
        Self { entries, get_name: Cell::new(get_name) }
    }

    /// The entries.
    pub fn entries(&self) -> &'static [E] {
        self.entries
    }

    /// Entry `idx`, if there is one (the C++ indexes unchecked).
    pub fn entry(&self, idx: usize) -> Option<&'static E> {
        self.entries.get(idx)
    }

    /// The naming function.
    pub fn get_name(&self) -> Option<&'static GetName> {
        self.get_name.get()
    }

    /// The number of entries.
    pub fn size(&self) -> usize {
        self.entries.len()
    }

    /// The name of entry `idx`.
    pub fn name(&self, idx: usize) -> Option<&'static [u8]> {
        self.get_name.get().and_then(|get_name| get_name(idx))
    }

    /// An iterator over the named entries.
    pub fn iter(&self) -> StatisticsIterator<E> {
        StatisticsIterator::new(self.get_name.get(), self.entries)
    }
}

impl<E: HasStatistics + 'static> StatisticsContainer<E> {
    /// The statistics of entry `idx`.
    pub fn statistics(&self, idx: usize) -> Option<&'static E::Statistics> {
        self.entries.get(idx).map(HasStatistics::statistics)
    }
}

impl<S: Statistics + 'static> StatisticsContainer<S> {
    /// Take the statistics and names of `src`, whose entries carry this container's
    /// statistics type.
    ///
    /// # Panics
    ///
    /// If `src` has more entries than this container (the C++ asserts).
    pub fn copy_from<E: HasStatistics<Statistics = S> + 'static>(
        &self,
        src: &StatisticsContainer<E>,
    ) {
        assert!(
            src.entries.len() <= self.entries.len(),
            "number of entries in source must fit into this container"
        );
        for (dst, entry) in self.entries.iter().zip(src.entries.iter()) {
            dst.copy_from(entry.statistics());
        }
        self.get_name.set(src.get_name.get());
    }

    /// Reset every entry.
    pub fn reset(&self) {
        for entry in self.entries {
            entry.reset();
        }
    }
}

/// Walks the named entries of a container: the port of `StatisticsIterator`.
pub struct StatisticsIterator<E: 'static> {
    get_name: Option<&'static GetName>,
    entries: &'static [E],
    current: usize,
}

impl<E: 'static> StatisticsIterator<E> {
    /// An iterator at the first named entry.
    pub fn new(get_name: Option<&'static GetName>, entries: &'static [E]) -> Self {
        let mut iterator = Self { get_name, entries, current: 0 };
        iterator.move_to_next();
        iterator
    }

    /// Advance to the next named entry.
    pub fn next(&mut self) {
        if self.current < self.entries.len() {
            self.current += 1;
            self.move_to_next();
        }
    }

    /// Back to the first named entry.
    pub fn reset(&mut self) {
        self.current = 0;
        self.move_to_next();
    }

    /// Whether the iterator is at an entry.
    pub fn has_value(&self) -> bool {
        self.current < self.entries.len()
    }

    /// The name of the current entry.
    pub fn name(&self) -> Option<&'static [u8]> {
        self.get_name.and_then(|get_name| get_name(self.current))
    }

    /// The current entry.
    pub fn statistics(&self) -> &'static E {
        &self.entries[self.current]
    }

    fn move_to_next(&mut self) {
        while self.current < self.entries.len() {
            if self.name().is_some() {
                return;
            }
            self.current += 1;
        }
    }
}

// Ported from runtime/test/src/StatisticsContainerTest.cpp and StatisticsIteratorTest.cpp.
#[cfg(test)]
mod tests {
    extern crate std;

    use std::boxed::Box;

    use super::*;
    use crate::stack::tests::Recorder;

    static NAMES: [Option<&[u8]>; 5] = [None, Some(b"abc"), None, Some(b"def"), None];
    fn name(idx: usize) -> Option<&'static [u8]> {
        NAMES[idx]
    }

    fn entries<const N: usize>() -> &'static [Recorder; N] {
        Box::leak(Box::new(core::array::from_fn(|_| Recorder::default())))
    }

    #[test]
    fn statistics_array() {
        let entries = entries::<5>();
        let cut = StatisticsContainer::new(entries, Some(&name));
        for idx in 0..5 {
            assert_eq!(cut.name(idx), NAMES[idx]);
            assert!(core::ptr::eq(cut.statistics(idx).unwrap(), &entries[idx]));
            assert!(core::ptr::eq(cut.entry(idx).unwrap(), &entries[idx]));
        }
        assert!(cut.entry(5).is_none());
        assert_eq!(cut.size(), 5);
        let mut it = cut.iter();
        assert!(it.has_value());
        assert_eq!(it.name(), Some(&b"abc"[..]));
        assert!(core::ptr::eq(it.statistics(), &entries[1]));
        it.next();
        assert!(it.has_value());
        assert_eq!(it.name(), Some(&b"def"[..]));
        assert!(core::ptr::eq(it.statistics(), &entries[3]));
        it.next();
        assert!(!it.has_value());
        it.next();
        assert!(!it.has_value());
        it.reset();
        assert!(it.has_value());
        assert_eq!(it.name(), Some(&b"abc"[..]));
    }

    #[test]
    fn copy_from_takes_statistics_and_names() {
        let source_entries: &'static [SimpleRuntimeEntry<Recorder, true>; 5] =
            Box::leak(Box::new(core::array::from_fn(|_| {
                SimpleRuntimeEntry::new(Recorder::default())
            })));
        source_entries[1].statistics.add_run(1, 2, 3);
        let source = StatisticsContainer::new(source_entries, Some(&name));
        let dest = StatisticsContainer::new(entries::<6>(), None);
        assert!(dest.name(1).is_none());
        dest.copy_from(&source);
        assert_eq!(dest.name(1), Some(&b"abc"[..]));
        assert_eq!(dest.statistics(1).unwrap().take(), [(1, 2, 3)]);
        assert!(dest.statistics(0).unwrap().take().is_empty());
        dest.reset();
        assert_eq!(*dest.statistics(2).unwrap().resets.borrow(), 1);
    }

    #[test]
    #[should_panic(expected = "number of entries")]
    fn copy_from_rejects_a_larger_source() {
        let source = StatisticsContainer::new(entries::<5>(), Some(&name));
        StatisticsContainer::new(entries::<4>(), None).copy_from(&source);
    }

    #[test]
    fn unnamed_container_iterates_nothing() {
        let cut = StatisticsContainer::new(entries::<3>(), None);
        assert_eq!(cut.size(), 3);
        assert!(!cut.iter().has_value());
        assert!(cut.get_name().is_none());
    }
}
