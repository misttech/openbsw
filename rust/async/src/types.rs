// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The async types, ported from the Zephyr adaptation's `async/Types.h` and
//! `asyncImpl`'s `IRunnable.h`.

use core::cell::Cell;

use openbsw_timer::{TimeoutNode, TimerLink};

use crate::queue::{HasQueueNode, QueueNode};

/// An async context: the index of the task that runs it, lower is higher priority.
pub type ContextType = u8;

/// No context.
pub const CONTEXT_INVALID: ContextType = 0xFF;

/// A set of event bits.
pub type EventMaskType = u32;

/// Something a context executes: the port of `IRunnable`.
pub trait Runnable: Sync {
    /// Run.
    fn execute(&self);

    /// The node that links this runnable into a context's queue.
    fn node(&self) -> &QueueNode<dyn Runnable>;
}

impl HasQueueNode for dyn Runnable {
    fn queue_node(&self) -> &QueueNode<dyn Runnable> {
        self.node()
    }
}

/// The units of a delay or period.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum TimeUnit {
    /// Microseconds.
    Microseconds = 1,
    /// Milliseconds.
    Milliseconds = 1000,
    /// Seconds.
    Seconds = 1_000_000,
}

impl TimeUnit {
    /// The number of microseconds in one unit.
    pub const fn microseconds(self) -> u32 {
        self as u32
    }
}

/// A scheduled execution of a runnable on a context: the port of `async::TimeoutType`.
///
/// The binding fills in the runnable and context when the timeout is scheduled; expiring
/// executes the runnable.
pub struct Timeout {
    link: TimerLink,
    runnable: Cell<Option<&'static dyn Runnable>>,
    context: Cell<ContextType>,
}

// SAFETY: the cells are written by the binding inside its critical section when the
// timeout is scheduled or cancelled, and read when it expires; the platform serializes
// the sharing, as it does for the C++ struct's plain fields.
unsafe impl Sync for Timeout {}

impl Timeout {
    /// A timeout that is not scheduled.
    pub const fn new() -> Self {
        Self { link: TimerLink::new(), runnable: Cell::new(None), context: Cell::new(0) }
    }

    /// Cancel the timeout through the binding; nothing happens if it is not scheduled.
    pub fn cancel(&'static self) {
        crate::binding::cancel(self);
    }

    /// The runnable to execute.
    pub fn runnable(&self) -> Option<&'static dyn Runnable> {
        self.runnable.get()
    }

    /// Set the runnable to execute.
    pub fn set_runnable(&self, runnable: Option<&'static dyn Runnable>) {
        self.runnable.set(runnable);
    }

    /// The context the timeout is scheduled on.
    pub fn context(&self) -> ContextType {
        self.context.get()
    }

    /// Set the context the timeout is scheduled on.
    pub fn set_context(&self, context: ContextType) {
        self.context.set(context);
    }
}

impl Default for Timeout {
    fn default() -> Self {
        Self::new()
    }
}

impl TimeoutNode for Timeout {
    fn link(&self) -> &TimerLink {
        &self.link
    }

    fn expired(&self) {
        if let Some(runnable) = self.runnable.get() {
            runnable.execute();
        }
    }
}
