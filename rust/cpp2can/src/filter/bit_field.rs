// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! One bit per base id, ported from `filter/BitFieldFilter.h` and `.cpp`.

use core::cell::Cell;

use super::{Filter, IntervalFilter, MaskFilter, Merger, StaticMask};
use crate::CanFrame;

/// A filter over the base ids, one bit each; also the merger every transceiver uses.
pub struct BitFieldFilter {
    mask: [Cell<u8>; Self::MASK_SIZE as usize],
}

// SAFETY: see the module documentation of `filter`: configured during startup or under
// the owning transceiver's lock.
unsafe impl Sync for BitFieldFilter {}

impl BitFieldFilter {
    /// The largest id the filter holds.
    pub const MAX_ID: u16 = CanFrame::MAX_FRAME_ID as u16;
    /// The number of ids.
    pub const NUMBER_OF_BITS: u16 = Self::MAX_ID + 1;
    /// The number of mask bytes.
    pub const MASK_SIZE: u16 = Self::NUMBER_OF_BITS / 8;

    /// An empty filter.
    pub const fn new() -> Self {
        Self { mask: [const { Cell::new(0) }; Self::MASK_SIZE as usize] }
    }

    /// Merge another bit field in.
    pub fn merge_with_bit_field(&self, filter: &BitFieldFilter) {
        for (mine, theirs) in self.mask.iter().zip(filter.mask.iter()) {
            mine.set(mine.get() | theirs.get());
        }
    }

    /// Merge a static bit field in.
    pub fn merge_with_static_bit_field(&self, filter: &dyn StaticMask) {
        for (index, mine) in self.mask.iter().enumerate() {
            mine.set(mine.get() | filter.mask_value(index as u16));
        }
    }

    /// Merge an interval in, cut at [`Self::MAX_ID`].
    pub fn merge_with_interval(&self, filter: &IntervalFilter) {
        if filter.lower_bound() <= filter.upper_bound() {
            let to_id = filter.upper_bound().min(u32::from(Self::MAX_ID));
            self.add_range(filter.lower_bound(), to_id);
        }
    }

    /// Merge a mask filter in: every base id it matches.
    pub fn merge_with_mask(&self, filter: &MaskFilter) {
        for id in 0..=u32::from(Self::MAX_ID) {
            if filter.matches(id) {
                self.add(id);
            }
        }
    }

    /// Byte `index` of the mask (`getRawBitField()[index]`).
    pub fn mask_byte(&self, index: u16) -> u8 {
        self.mask[usize::from(index)].get()
    }
}

impl Default for BitFieldFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for BitFieldFilter {
    fn eq(&self, other: &Self) -> bool {
        self.mask.iter().zip(other.mask.iter()).all(|(a, b)| a.get() == b.get())
    }
}

impl Filter for BitFieldFilter {
    fn add(&self, filter_id: u32) {
        if filter_id <= u32::from(Self::MAX_ID) {
            let cell = &self.mask[(filter_id / 8) as usize];
            cell.set(cell.get() | (1 << (filter_id % 8)) as u8);
        }
    }

    fn add_range(&self, from: u32, to: u32) {
        for id in from..=to {
            self.add(id);
        }
    }

    fn matches(&self, filter_id: u32) -> bool {
        if filter_id <= u32::from(Self::MAX_ID) {
            let bit = (1 << (filter_id % 8)) as u8;
            (self.mask[(filter_id / 8) as usize].get() & bit) == bit
        } else {
            false
        }
    }

    fn clear(&self) {
        for cell in &self.mask {
            cell.set(0);
        }
    }

    fn open(&self) {
        for cell in &self.mask {
            cell.set(0xFF);
        }
    }

    fn accept_merger(&self, merger: &dyn Merger) {
        merger.merge_with_bit_field(self);
    }
}

impl Merger for BitFieldFilter {
    fn merge_with_bit_field(&self, filter: &BitFieldFilter) {
        BitFieldFilter::merge_with_bit_field(self, filter);
    }

    fn merge_with_static_bit_field(&self, filter: &dyn StaticMask) {
        BitFieldFilter::merge_with_static_bit_field(self, filter);
    }

    fn merge_with_interval(&self, filter: &IntervalFilter) {
        BitFieldFilter::merge_with_interval(self, filter);
    }

    fn merge_with_mask(&self, filter: &MaskFilter) {
        BitFieldFilter::merge_with_mask(self, filter);
    }
}

// Ported from cpp2can/test/src/can/filter/BitFieldFilterTest.cpp.
#[cfg(test)]
mod tests {
    extern crate std;

    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn add_id() {
        let filter = BitFieldFilter::new();
        let (id1, id2, id3, id4) = (0x00, 0xFF, 0x6F1, 0x7FF);
        assert!(!filter.matches(id1));
        filter.add(id1);
        assert!(filter.matches(id1));
        assert!(!filter.matches(id2));
        filter.add(id2);
        assert!(filter.matches(id1));
        assert!(filter.matches(id2));
        assert!(!filter.matches(id3));
        filter.add(id3);
        assert!(filter.matches(id1));
        assert!(filter.matches(id2));
        assert!(filter.matches(id3));
        assert!(!filter.matches(id4));
        filter.add(id4);
        assert!(filter.matches(id1));
        assert!(filter.matches(id2));
        assert!(filter.matches(id3));
        assert!(filter.matches(id4));
        // Extended ids are neither added nor matched.
        filter.add(0x80000000);
        assert!(!filter.matches(0x80000000));
        filter.add(0x90000000);
        assert!(!filter.matches(0x90000000));
    }

    #[test]
    fn add_range() {
        let filter = BitFieldFilter::new();
        let to = u32::from(BitFieldFilter::MAX_ID);
        for id in 0..=to {
            assert!(!filter.matches(id));
        }
        filter.add_range(0, to);
        for id in 0..=to {
            assert!(filter.matches(id));
        }
        assert!(!filter.matches(to + 1));
    }

    // An empty or inverted range adds nothing, as the C++ loop `for (i = from; i <= to; ++i)`
    // does not run; merging an interval of extended ids adds no base id.
    #[test]
    fn add_range_with_from_above_to_adds_nothing() {
        let filter = BitFieldFilter::new();
        filter.add_range(0x7FF, 0x100);
        filter.add_range(0x900, 0x7FF);
        for id in 0..=u32::from(BitFieldFilter::MAX_ID) {
            assert!(!filter.matches(id));
        }
        filter.merge_with_interval(&IntervalFilter::with_range(0x1000, 0x2000));
        for id in 0..=u32::from(BitFieldFilter::MAX_ID) {
            assert!(!filter.matches(id));
        }
    }

    #[test]
    fn merge_with_bit_field_filter() {
        let filter1 = BitFieldFilter::new();
        let filter2 = BitFieldFilter::new();
        let expected = BitFieldFilter::new();
        let ids = [0x000, 0x001, 0x100, 0x123, 0x234, 0x345];
        for id in &ids[..3] {
            filter1.add(*id);
        }
        for id in &ids[2..] {
            filter2.add(*id);
        }
        for id in &ids {
            expected.add(*id);
        }
        filter1.accept_merger(&filter2);
        // accept_merger merged filter1 into filter2; the C++ test merges 2 into 1.
        filter2.merge_with_bit_field(&filter1);
        filter1.merge_with_bit_field(&filter2);
        assert!(filter1 == expected);
        for id in &ids {
            assert!(filter1.matches(*id));
            assert!(expected.matches(*id));
        }
        filter1.add(0x456);
        assert!(filter1 != expected);
    }

    #[test]
    fn merge_with_interval_filter() {
        let filter = BitFieldFilter::new();
        let interval = IntervalFilter::new();
        // 100 pseudo-random base ids, from a fixed-seed generator in place of rand().
        let mut state: u32 = 12345;
        let mut random_ids = BTreeSet::new();
        while random_ids.len() < 100 {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let id = (state >> 16) % u32::from(BitFieldFilter::MAX_ID) + 1;
            random_ids.insert(id);
            filter.add(id);
        }
        for id in &random_ids {
            assert!(filter.matches(*id));
        }
        let (from, to) = (0x100, 0x234);
        interval.add_range(from, to);
        filter.merge_with_interval(&interval);
        for id in &random_ids {
            assert!(filter.matches(*id));
        }
        for id in from..=to {
            assert!(filter.matches(id));
        }
        interval.add_range(0x0, CanFrame::MAX_FRAME_ID);
        filter.merge_with_interval(&interval);
        for id in 0..=CanFrame::MAX_FRAME_ID {
            assert!(filter.matches(id));
        }
        // An interval reaching into the extended ids is cut at the base range.
        interval.add_range(0x0, CanFrame::MAX_FRAME_ID_EXTENDED);
        filter.merge_with_interval(&interval);
        assert!(!filter.matches(CanFrame::MAX_FRAME_ID + 1));
    }

    #[test]
    fn merge_with_mask_filter() {
        let filter = BitFieldFilter::new();
        filter.add(0x123);
        let mask = MaskFilter::new();
        mask.add(0x0FF);
        filter.merge_with_mask(&mask);
        assert!(filter.matches(0x123));
        assert!(filter.matches(0x0FF));
        assert!(!filter.matches(0x100));
        mask.open();
        filter.merge_with_mask(&mask);
        for id in 0..=CanFrame::MAX_FRAME_ID {
            assert!(filter.matches(id));
        }
    }

    #[test]
    fn clear() {
        let filter = BitFieldFilter::new();
        let to = u32::from(BitFieldFilter::MAX_ID);
        filter.open();
        for id in 0..=to {
            assert!(filter.matches(id));
        }
        filter.clear();
        for id in 0..=to {
            assert!(!filter.matches(id));
        }
    }

    #[test]
    fn open() {
        let filter = BitFieldFilter::new();
        let to = u32::from(BitFieldFilter::MAX_ID);
        filter.clear();
        for id in 0..=to {
            assert!(!filter.matches(id));
        }
        filter.open();
        for id in 0..=to {
            assert!(filter.matches(id));
        }
    }

    #[test]
    fn matches_nothing_above_the_largest_id() {
        let filter = BitFieldFilter::new();
        assert!(!filter.matches(u32::from(BitFieldFilter::MAX_ID) + 1));
    }

    #[test]
    fn raw_bit_field() {
        let filter = BitFieldFilter::new();
        filter.add(0x009);
        assert_eq!(filter.mask_byte(1), 0x02);
        assert_eq!(filter.mask_byte(0), 0);
    }
}
