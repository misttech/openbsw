// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! A mask and pattern, ported from `filter/MaskFilter.h` and `.cpp`.

use core::cell::Cell;

use super::{Filter, Merger};

/// A filter accepting every id that equals a pattern under a mask; adding ids widens the
/// mask to the bits they share.
pub struct MaskFilter {
    mask: Cell<u32>,
    pattern: Cell<u32>,
    empty: Cell<bool>,
}

// SAFETY: see the module documentation of `filter`.
unsafe impl Sync for MaskFilter {}

impl MaskFilter {
    /// An empty filter.
    pub const fn new() -> Self {
        Self { mask: Cell::new(0), pattern: Cell::new(0), empty: Cell::new(true) }
    }

    /// A filter accepting `from` to `to`.
    pub fn with_range(from: u32, to: u32) -> Self {
        let filter = Self::new();
        filter.add_range(from, to);
        filter
    }

    /// The mask.
    pub fn mask(&self) -> u32 {
        self.mask.get()
    }

    /// The pattern.
    pub fn pattern(&self) -> u32 {
        self.pattern.get()
    }

    fn widen(&self, mask: u32, pattern: u32) {
        if self.empty.get() {
            self.mask.set(mask);
            self.pattern.set(pattern & mask);
            self.empty.set(false);
        } else {
            let common_mask = self.mask.get() & mask & !(self.pattern.get() ^ pattern);
            self.mask.set(common_mask);
            self.pattern.set(self.pattern.get() & common_mask);
        }
    }
}

impl Default for MaskFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl Filter for MaskFilter {
    fn add(&self, filter_id: u32) {
        self.widen(0xFFFF_FFFF, filter_id);
    }

    fn add_range(&self, mut from: u32, mut to: u32) {
        if from > to {
            core::mem::swap(&mut from, &mut to);
        }
        let mut block_mask: u32 = 0xFFFF_FFFF;
        let mut difference = from ^ to;
        while difference != 0 {
            block_mask <<= 1;
            difference >>= 1;
        }
        self.widen(block_mask, from & block_mask);
    }

    fn matches(&self, filter_id: u32) -> bool {
        !self.empty.get() && (filter_id & self.mask.get()) == self.pattern.get()
    }

    fn clear(&self) {
        self.mask.set(0);
        self.pattern.set(0);
        self.empty.set(true);
    }

    fn open(&self) {
        self.mask.set(0);
        self.pattern.set(0);
        self.empty.set(false);
    }

    fn accept_merger(&self, merger: &dyn Merger) {
        merger.merge_with_mask(self);
    }
}

// Ported from cpp2can/test/src/can/filter/MaskFilterTest.cpp.
#[cfg(test)]
mod tests {
    use super::super::{BitFieldFilter, IntervalFilter, StaticMask};
    use super::*;

    #[test]
    fn default_constructor() {
        let filter = MaskFilter::new();
        assert!(!filter.matches(0x0));
        assert!(!filter.matches(0x7FF));
        assert!(!filter.matches(0xFFFFFFFF));
    }

    #[test]
    fn add_id() {
        let filter = MaskFilter::new();
        filter.add(0x123);
        assert!(filter.matches(0x123));
        assert!(!filter.matches(0x122));
        assert!(!filter.matches(0x124));
        assert_eq!(filter.mask(), 0xFFFFFFFF);
        assert_eq!(filter.pattern(), 0x123);
        filter.add(0x120);
        assert!(filter.matches(0x123));
        assert!(filter.matches(0x120));
        assert!(filter.matches(0x121));
        assert!(filter.matches(0x122));
        assert!(!filter.matches(0x124));
        assert!(!filter.matches(0x11F));
    }

    #[test]
    fn add_range() {
        let filter = MaskFilter::new();
        filter.add_range(0x700, 0x7FF);
        for id in 0x700..=0x7FF {
            assert!(filter.matches(id));
        }
        assert!(!filter.matches(0x6FF));
        assert!(!filter.matches(0x800));
        assert_eq!(filter.mask(), 0xFFFFFF00);
        assert_eq!(filter.pattern(), 0x700);
    }

    #[test]
    fn add_range_differing_in_single_bit() {
        let filter = MaskFilter::new();
        filter.add_range(0x98DA0000, 0x98DB0000);
        assert_eq!(filter.mask(), 0xFFFE0000);
        assert_eq!(filter.pattern(), 0x98DA0000);
        assert!(filter.matches(0x98DA0000));
        assert!(filter.matches(0x98DA1234));
        assert!(filter.matches(0x98DBFFFF));
        assert!(!filter.matches(0x98DC0000));
        assert!(!filter.matches(0x18DA0000));
    }

    #[test]
    fn add_range_swapped() {
        let filter = MaskFilter::new();
        filter.add_range(0x7FF, 0x700);
        for id in 0x700..=0x7FF {
            assert!(filter.matches(id));
        }
        assert!(!filter.matches(0x6FF));
    }

    #[test]
    fn range_constructor() {
        let filter = MaskFilter::with_range(0x700, 0x7FF);
        for id in 0x700..=0x7FF {
            assert!(filter.matches(id));
        }
        assert!(!filter.matches(0x6FF));
        assert!(!filter.matches(0x800));
    }

    #[test]
    fn clear() {
        let filter = MaskFilter::new();
        filter.add_range(0x700, 0x7FF);
        assert!(filter.matches(0x700));
        filter.clear();
        assert!(!filter.matches(0x700));
        assert!(!filter.matches(0x0));
    }

    #[test]
    fn open() {
        let filter = MaskFilter::new();
        filter.open();
        assert!(filter.matches(0x0));
        assert!(filter.matches(0x7FF));
        assert!(filter.matches(0xFFFFFFFF));
    }

    #[test]
    fn accept_merger() {
        struct MergerStub(Cell<bool>);
        impl Merger for MergerStub {
            fn merge_with_bit_field(&self, _: &BitFieldFilter) {}
            fn merge_with_static_bit_field(&self, _: &dyn StaticMask) {}
            fn merge_with_interval(&self, _: &IntervalFilter) {}
            fn merge_with_mask(&self, _: &MaskFilter) {
                self.0.set(true);
            }
        }
        let merger = MergerStub(Cell::new(false));
        MaskFilter::new().accept_merger(&merger);
        assert!(merger.0.get());
    }
}
