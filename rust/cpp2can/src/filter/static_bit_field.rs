// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! A read-only bit field, ported from `filter/AbstractStaticBitFieldFilter.h` and `.cpp`.

use super::{Filter, Merger};
use crate::CanFrame;

/// The bytes of a static bit field: the port of the pure virtual `getMaskValue`.
pub trait StaticMask: Sync {
    /// Byte `byte_index` of the mask, bit `n` standing for id `byte_index * 8 + n`.
    fn mask_value(&self, byte_index: u16) -> u8;
}

/// A filter over a bit field that is fixed: `add`, `clear` and `open` do nothing.
pub struct StaticBitFieldFilter<M: StaticMask> {
    mask: M,
}

impl<M: StaticMask> StaticBitFieldFilter<M> {
    /// The largest id the filter holds.
    pub const MAX_ID: u16 = CanFrame::MAX_FRAME_ID as u16;
    /// The number of ids.
    pub const NUMBER_OF_BITS: u16 = Self::MAX_ID + 1;
    /// The number of mask bytes.
    pub const MASK_SIZE: u16 = Self::NUMBER_OF_BITS / 8;

    /// A filter over `mask`.
    pub const fn new(mask: M) -> Self {
        Self { mask }
    }

    /// The mask.
    pub const fn mask(&self) -> &M {
        &self.mask
    }
}

/// Two static bit fields are equal when every mask byte is.
pub fn static_bit_fields_equal(x: &dyn StaticMask, y: &dyn StaticMask) -> bool {
    (0..CanFrame::MAX_FRAME_ID as u16 / 8 + 1).all(|i| x.mask_value(i) == y.mask_value(i))
}

impl<M: StaticMask> Filter for StaticBitFieldFilter<M> {
    fn add(&self, _filter_id: u32) {}

    fn add_range(&self, _from: u32, _to: u32) {}

    fn matches(&self, filter_id: u32) -> bool {
        if filter_id <= u32::from(Self::MAX_ID) {
            let bit = (1 << (filter_id % 8)) as u8;
            (self.mask.mask_value((filter_id / 8) as u16) & bit) == bit
        } else {
            false
        }
    }

    fn clear(&self) {}

    fn open(&self) {}

    fn accept_merger(&self, merger: &dyn Merger) {
        merger.merge_with_static_bit_field(&self.mask);
    }
}

#[cfg(test)]
mod tests {
    use core::cell::Cell;

    use super::super::BitFieldFilter;
    use super::*;

    struct ByteMask(Cell<u8>);
    // SAFETY: single-threaded test object.
    unsafe impl Sync for ByteMask {}
    impl StaticMask for ByteMask {
        fn mask_value(&self, _byte_index: u16) -> u8 {
            self.0.get()
        }
    }

    #[test]
    fn matches_the_fixed_mask_and_ignores_changes() {
        let filter = StaticBitFieldFilter::new(ByteMask(Cell::new(0x01)));
        assert!(filter.matches(0x000));
        assert!(filter.matches(0x008));
        assert!(!filter.matches(0x001));
        assert!(!filter.matches(0x800));
        filter.add(0x001);
        filter.add_range(0, 0x7FF);
        filter.open();
        assert!(!filter.matches(0x001));
        filter.clear();
        assert!(filter.matches(0x000));
        filter.mask().0.set(0xFF);
        assert!(filter.matches(0x001));
    }

    #[test]
    fn merges_into_a_bit_field() {
        let filter = StaticBitFieldFilter::new(ByteMask(Cell::new(0x80)));
        let merged = BitFieldFilter::new();
        filter.accept_merger(&merged);
        assert!(merged.matches(0x007));
        assert!(merged.matches(0x7FF));
        assert!(!merged.matches(0x006));
        assert!(static_bit_fields_equal(filter.mask(), &ByteMask(Cell::new(0x80))));
        assert!(!static_bit_fields_equal(filter.mask(), &ByteMask(Cell::new(0x00))));
    }
}
