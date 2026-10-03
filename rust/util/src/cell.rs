// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! A cell for global state that is written only while nothing else runs.
//!
//! OpenBSW's C++ keeps plenty of global, non-atomic state that is set up before the tasks
//! start (`Logger::init`, the sorted command list, component indices) and only read after.
//! [`RacyCell`] is the Rust spelling of that contract: the caller, not the type, guarantees
//! that no read overlaps a write.

use core::cell::UnsafeCell;

/// A value shared between threads whose accesses the caller serializes.
pub struct RacyCell<T>(UnsafeCell<T>);

// SAFETY: every access goes through `get` or `set`, whose contracts forbid overlapping
// accesses from different threads; the type adds no sharing of its own.
unsafe impl<T: Send> Sync for RacyCell<T> {}

impl<T> RacyCell<T> {
    /// A cell holding `value`.
    pub const fn new(value: T) -> Self {
        Self(UnsafeCell::new(value))
    }

    /// Read the value.
    ///
    /// # Safety
    ///
    /// No call to [`set`](Self::set) may be in progress on another thread, and the
    /// returned reference must not outlive the next `set`.
    pub unsafe fn get(&self) -> &T {
        // SAFETY: the caller guarantees no concurrent or later `set` while the reference
        // lives, so the shared reference does not alias a mutation.
        unsafe { &*self.0.get() }
    }

    /// Replace the value.
    ///
    /// # Safety
    ///
    /// No other access, `get` or `set`, may be in progress on any thread, and no
    /// reference returned by an earlier `get` may still be alive.
    pub unsafe fn set(&self, value: T) {
        // SAFETY: the caller guarantees exclusive access for the duration of the write.
        unsafe { *self.0.get() = value };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_and_returns_the_value() {
        static CELL: RacyCell<u32> = RacyCell::new(1);
        // SAFETY: single-threaded test.
        unsafe {
            assert_eq!(*CELL.get(), 1);
            CELL.set(2);
            assert_eq!(*CELL.get(), 2);
        }
    }
}
