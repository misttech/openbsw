// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The id filters, ported from `filter/`: what a listener accepts, and how a transceiver
//! merges every listener's filter into its own acceptance mask.
//!
//! A filter is configured through `&self`: listeners are `static` objects, so a filter is
//! set up during startup, or under the transceiver's lock when the transceiver merges it.
//! That is the C++ contract too; the cells are not shared between threads otherwise.

mod bit_field;
mod interval;
mod mask;
mod static_bit_field;

pub use bit_field::BitFieldFilter;
pub use interval::IntervalFilter;
pub use mask::MaskFilter;
pub use static_bit_field::{StaticBitFieldFilter, StaticMask, static_bit_fields_equal};

/// A set of CAN ids: the port of `IFilter`.
pub trait Filter {
    /// Add one id.
    fn add(&self, filter_id: u32);
    /// Add every id from `from` to `to`, both included.
    fn add_range(&self, from: u32, to: u32);
    /// Whether `filter_id` is in the set.
    fn matches(&self, filter_id: u32) -> bool;
    /// Remove every id.
    fn clear(&self);
    /// Add every id.
    fn open(&self);
    /// Fold this filter into `merger` (the visitor of `IMerger`).
    fn accept_merger(&self, merger: &dyn Merger);
}

/// Something that takes filters in: the port of `IMerger`.
pub trait Merger {
    /// Merge a [`BitFieldFilter`].
    fn merge_with_bit_field(&self, filter: &BitFieldFilter);
    /// Merge a [`StaticBitFieldFilter`].
    fn merge_with_static_bit_field(&self, filter: &dyn StaticMask);
    /// Merge an [`IntervalFilter`].
    fn merge_with_interval(&self, filter: &IntervalFilter);
    /// Merge a [`MaskFilter`].
    fn merge_with_mask(&self, filter: &MaskFilter);
}
