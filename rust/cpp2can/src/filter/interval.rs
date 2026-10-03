// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! One contiguous id range, ported from `filter/IntervalFilter.h` and `.cpp`.

use core::cell::Cell;

use super::{Filter, Merger};
use crate::{CanFrame, can_id};

/// A filter accepting every id from a lower to an upper bound.
pub struct IntervalFilter {
    from: Cell<u32>,
    to: Cell<u32>,
}

// SAFETY: see the module documentation of `filter`.
unsafe impl Sync for IntervalFilter {}

impl IntervalFilter {
    /// The largest id: the extended id with every identifier bit set.
    pub const MAX_ID: u32 = can_id::extended(CanFrame::MAX_FRAME_ID_EXTENDED);

    /// An empty filter.
    pub const fn new() -> Self {
        Self { from: Cell::new(Self::MAX_ID), to: Cell::new(0) }
    }

    /// A filter accepting `from` to `to`.
    pub const fn with_range(from: u32, to: u32) -> Self {
        let (from, to) = Self::clamp(from, to);
        Self { from: Cell::new(from), to: Cell::new(to) }
    }

    /// The lower bound.
    pub fn lower_bound(&self) -> u32 {
        self.from.get()
    }

    /// The upper bound.
    pub fn upper_bound(&self) -> u32 {
        self.to.get()
    }

    /// `from` and `to` cut at [`Self::MAX_ID`] and ordered.
    const fn clamp(mut from: u32, mut to: u32) -> (u32, u32) {
        if from > Self::MAX_ID {
            from = Self::MAX_ID;
        }
        if to > Self::MAX_ID {
            to = Self::MAX_ID;
        }
        if from > to {
            core::mem::swap(&mut from, &mut to);
        }
        (from, to)
    }
}

impl Default for IntervalFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl Filter for IntervalFilter {
    fn add(&self, filter_id: u32) {
        self.add_range(filter_id, filter_id);
    }

    fn add_range(&self, from: u32, to: u32) {
        let (from, to) = Self::clamp(from, to);
        self.from.set(self.from.get().min(from));
        self.to.set(self.to.get().max(to));
    }

    fn matches(&self, filter_id: u32) -> bool {
        filter_id >= self.from.get() && filter_id <= self.to.get()
    }

    fn clear(&self) {
        self.from.set(Self::MAX_ID);
        self.to.set(0);
    }

    fn open(&self) {
        self.from.set(0);
        self.to.set(Self::MAX_ID);
    }

    fn accept_merger(&self, merger: &dyn Merger) {
        merger.merge_with_interval(self);
    }
}

// Ported from cpp2can/test/src/can/filter/IntervalFilterTest.cpp.
#[cfg(test)]
mod tests {
    use super::*;

    fn verify_range(filter: &IntervalFilter, from: u32, to: u32) {
        for id in 0..from {
            assert!(!filter.matches(id), "{id:#x} matched below {from:#x}");
        }
        for id in from..=to {
            assert!(filter.matches(id), "{id:#x} did not match in {from:#x}..={to:#x}");
        }
        for id in to + 1..=CanFrame::MAX_FRAME_ID {
            assert!(!filter.matches(id), "{id:#x} matched above {to:#x}");
        }
    }

    #[test]
    fn default_constructor() {
        let filter = IntervalFilter::new();
        for id in 0..=CanFrame::MAX_FRAME_ID {
            assert!(!filter.matches(id));
        }
        assert_eq!(filter.lower_bound(), IntervalFilter::MAX_ID);
        assert_eq!(filter.upper_bound(), 0);
    }

    #[test]
    fn init_constructor() {
        let filter1 = IntervalFilter::with_range(0x0, CanFrame::MAX_FRAME_ID);
        assert_eq!(filter1.lower_bound(), 0);
        assert_eq!(filter1.upper_bound(), CanFrame::MAX_FRAME_ID);
        verify_range(&filter1, 0x0, CanFrame::MAX_FRAME_ID);
        let (from, to) = (0x111, 0x345);
        let filter2 = IntervalFilter::with_range(from, to);
        assert_eq!(filter2.lower_bound(), from);
        assert_eq!(filter2.upper_bound(), to);
        verify_range(&filter2, from, to);
        let filter3 = IntervalFilter::with_range(0x0, 0x80000000);
        assert_eq!(filter3.upper_bound(), 0x80000000);
        let filter4 = IntervalFilter::with_range(0x80000000, 0x90000000);
        assert_eq!(filter4.lower_bound(), 0x80000000);
        assert_eq!(filter4.upper_bound(), 0x90000000);
    }

    #[test]
    fn add_id() {
        let filter = IntervalFilter::new();
        let (id1, id2, id3) = (0x00, 0xFF, 0x6F1);
        assert!(!filter.matches(id1));
        assert!(!filter.matches(id2));
        assert!(!filter.matches(id3));
        filter.add(id1);
        assert!(filter.matches(id1));
        assert!(!filter.matches(id2));
        assert!(!filter.matches(id3));
        filter.add(id2);
        assert!(filter.matches(id1));
        assert!(filter.matches(id2));
        assert!(!filter.matches(id3));
        filter.add(id3);
        verify_range(&filter, id1, id3);
        filter.add(CanFrame::MAX_FRAME_ID);
        assert!(filter.matches(CanFrame::MAX_FRAME_ID));
        verify_range(&filter, id1, CanFrame::MAX_FRAME_ID);
        filter.add(0x90000000);
        verify_range(&filter, id1, CanFrame::MAX_FRAME_ID);
    }

    #[test]
    fn add_range() {
        let filter = IntervalFilter::new();
        let (mut from, mut to) = (0x100, 0x1FF);
        for id in from..=to {
            assert!(!filter.matches(id));
        }
        filter.add_range(from, to);
        assert_eq!((filter.lower_bound(), filter.upper_bound()), (from, to));
        verify_range(&filter, from, to);
        from = 0x90;
        filter.add_range(from, to);
        assert_eq!((filter.lower_bound(), filter.upper_bound()), (from, to));
        verify_range(&filter, from, to);
        to = 0x2FF;
        filter.add_range(from, to);
        assert_eq!((filter.lower_bound(), filter.upper_bound()), (from, to));
        verify_range(&filter, from, to);
        from = 0x50;
        to = 0x3FF;
        filter.add_range(from, to);
        assert_eq!((filter.lower_bound(), filter.upper_bound()), (from, to));
        verify_range(&filter, from, to);
        filter.add_range(from + 1, to);
        assert_eq!((filter.lower_bound(), filter.upper_bound()), (from, to));
        filter.add_range(from, to - 1);
        assert_eq!((filter.lower_bound(), filter.upper_bound()), (from, to));
        filter.add_range(from + 1, to - 1);
        assert_eq!((filter.lower_bound(), filter.upper_bound()), (from, to));
        verify_range(&filter, from, to);
        // Swapped bounds.
        from = 0x600;
        to = 0x10;
        filter.add_range(from, to);
        assert_eq!((filter.lower_bound(), filter.upper_bound()), (to, from));
        verify_range(&filter, to, from);
        // Bounds beyond the largest id are cut.
        let from2 = 0x10;
        let to2 = 0xB0000000;
        filter.add_range(from2, to2);
        assert_eq!(filter.lower_bound(), from2);
        assert_eq!(filter.upper_bound(), IntervalFilter::MAX_ID);
        filter.clear();
        filter.add_range(0xA0000000, to2);
        assert_eq!(filter.lower_bound(), IntervalFilter::MAX_ID);
        filter.clear();
    }

    #[test]
    fn clear() {
        let filter = IntervalFilter::new();
        let (id1, id2, id3) = (0x00, 0xFF, 0x6F1);
        filter.add_range(id1, id2);
        filter.add(id3);
        verify_range(&filter, id1, id3);
        filter.clear();
        for id in 0..=CanFrame::MAX_FRAME_ID {
            assert!(!filter.matches(id));
        }
    }

    #[test]
    fn open() {
        let filter = IntervalFilter::new();
        filter.clear();
        for id in 0..=CanFrame::MAX_FRAME_ID {
            assert!(!filter.matches(id));
        }
        filter.open();
        for id in 0..=CanFrame::MAX_FRAME_ID {
            assert!(filter.matches(id));
        }
        assert!(filter.matches(IntervalFilter::MAX_ID));
    }

    #[test]
    fn matches_nothing_above_the_largest_id() {
        let filter = IntervalFilter::new();
        filter.open();
        assert!(!filter.matches(IntervalFilter::MAX_ID + 1));
    }
}
