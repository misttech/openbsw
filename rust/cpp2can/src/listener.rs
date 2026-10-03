// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Who hears about frames, ported from `framemgmt/ICANFrameListener.h`,
//! `framemgmt/IFilteredCANFrameSentListener.h` and `canframes/ICANFrameSentListener.h`.
//!
//! Listeners are `'static` objects linked into a transceiver's intrusive lists through the
//! nodes they embed, as the C++ listeners are through `etl::bidirectional_link` and
//! `etl::forward_link`.

use core::cell::Cell;

use crate::CanFrame;
use crate::filter::Filter;

/// Receives the frames its filter accepts: the port of `ICANFrameListener`.
pub trait CanFrameListener: Sync {
    /// A frame the filter accepted arrived.
    fn frame_received(&self, frame: &CanFrame);
    /// The ids this listener wants.
    fn filter(&self) -> &dyn Filter;
    /// The node that links this listener into a transceiver's list.
    fn node(&self) -> &ListenerNode;
}

/// Told once about one frame it asked to send: the port of `ICANFrameSentListener`.
pub trait CanFrameSentListener: Sync {
    /// `frame` left the controller.
    fn can_frame_sent(&self, frame: &CanFrame);
}

/// Told about every frame a transceiver sends: the port of `IFilteredCANFrameSentListener`.
pub trait FilteredCanFrameSentListener: Sync {
    /// `frame` left the controller.
    fn can_frame_sent(&self, frame: &CanFrame);
    /// The ids this listener wants (kept for the interface; the transceiver base notifies
    /// every sent listener, as the C++ one does).
    fn filter(&self) -> &dyn Filter;
    /// The node that links this listener into a transceiver's list.
    fn node(&self) -> &SentListenerNode;
}

/// The link of a [`CanFrameListener`].
pub struct ListenerNode {
    pub(crate) next: Cell<Option<&'static dyn CanFrameListener>>,
}

// SAFETY: the list is changed and walked only under the owning transceiver's lock, or
// (notification) on the receiving context, as the C++ list is.
unsafe impl Sync for ListenerNode {}

impl ListenerNode {
    /// An unlinked node.
    pub const fn new() -> Self {
        Self { next: Cell::new(None) }
    }
}

impl Default for ListenerNode {
    fn default() -> Self {
        Self::new()
    }
}

/// The link of a [`FilteredCanFrameSentListener`].
pub struct SentListenerNode {
    pub(crate) next: Cell<Option<&'static dyn FilteredCanFrameSentListener>>,
}

// SAFETY: as `ListenerNode`.
unsafe impl Sync for SentListenerNode {}

impl SentListenerNode {
    /// An unlinked node.
    pub const fn new() -> Self {
        Self { next: Cell::new(None) }
    }
}

impl Default for SentListenerNode {
    fn default() -> Self {
        Self::new()
    }
}
