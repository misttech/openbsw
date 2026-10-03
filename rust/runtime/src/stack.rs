// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The stack of what is running, ported from `RuntimeStackEntry.h`,
//! `SimpleRuntimeEntry.h`, `NestedRuntimeEntry.h` and `RuntimeStack.h`.
//!
//! An entry is pushed when its context starts running and popped when it stops; the time
//! in between, less the time nested entries ran (`CUT_OUT`), is one run of its statistics.
//! Entries are `static`s linked by reference, as the C++ entries are by pointer.

use core::cell::Cell;

use crate::statistics::Statistics;

/// An entry a [`RuntimeStack`] holds: the methods the C++ CRTP base calls on `Entry`.
pub trait StackEntry: Sync + 'static {
    /// Whether the entry is on a stack.
    fn is_used(&self) -> bool;
    /// Start a run at `now`, on top of `prev`.
    fn push(&self, now: u32, prev: Option<&'static Self>);
    /// End the run at `now`; returns the entry below.
    fn pop(&self, now: u32) -> Option<&'static Self>;
    /// Account `suspended_time` of this run to what preempted it.
    fn add_suspended_time(&self, suspended_time: u32);
}

/// The run in progress: the port of `RuntimeStackEntry`'s fields.
struct EntryState<E: ?Sized + 'static> {
    prev: Cell<Option<&'static E>>,
    used: Cell<bool>,
    start_timestamp: Cell<u32>,
    suspended_time: Cell<u32>,
}

impl<E: ?Sized + 'static> EntryState<E> {
    const fn new() -> Self {
        Self {
            prev: Cell::new(None),
            used: Cell::new(false),
            start_timestamp: Cell::new(0),
            suspended_time: Cell::new(0),
        }
    }

    fn push(&self, now: u32, prev: Option<&'static E>) {
        if !self.used.get() {
            self.prev.set(prev);
            self.start_timestamp.set(now);
            self.suspended_time.set(0);
            self.used.set(true);
        }
    }

    /// Ends the run: `(start, runtime, suspended, prev)`.
    fn pop(&self, now: u32) -> (u32, u32, u32, Option<&'static E>) {
        let start = self.start_timestamp.get();
        let suspended = self.suspended_time.get();
        let runtime = now.wrapping_sub(start).wrapping_sub(suspended);
        self.used.set(false);
        (start, runtime, suspended, self.prev.take())
    }
}

/// An entry whose statistics are its own run time (`SimpleRuntimeEntry`); with `CUT_OUT`,
/// its run time is taken out of the entry below.
pub struct SimpleRuntimeEntry<S: Statistics, const CUT_OUT: bool> {
    state: EntryState<Self>,
    /// The statistics of this entry's runs.
    pub statistics: S,
}

// SAFETY: changed under the monitor's lock, as the C++ entries are.
unsafe impl<S: Statistics, const CUT_OUT: bool> Sync for SimpleRuntimeEntry<S, CUT_OUT> {}

impl<S: Statistics, const CUT_OUT: bool> SimpleRuntimeEntry<S, CUT_OUT> {
    /// An unused entry over `statistics`.
    pub const fn new(statistics: S) -> Self {
        Self { state: EntryState::new(), statistics }
    }
}

impl<S: Statistics + 'static, const CUT_OUT: bool> StackEntry for SimpleRuntimeEntry<S, CUT_OUT> {
    fn is_used(&self) -> bool {
        self.state.used.get()
    }

    fn push(&self, now: u32, prev: Option<&'static Self>) {
        self.state.push(now, prev);
    }

    fn pop(&self, now: u32) -> Option<&'static Self> {
        let (start, runtime, suspended, prev) = self.state.pop(now);
        self.statistics.add_run(start, runtime, suspended);
        if let Some(prev) = prev {
            prev.add_suspended_time(suspended.wrapping_add(if CUT_OUT { runtime } else { 0 }));
        }
        prev
    }

    fn add_suspended_time(&self, suspended_time: u32) {
        self.state.suspended_time.set(self.state.suspended_time.get().wrapping_add(suspended_time));
    }
}

/// An entry that is itself a stack of `N` entries (`NestedRuntimeEntry`): a context whose
/// functions are measured too. The nested stack is suspended while the entry is not on
/// top.
pub struct NestedRuntimeEntry<S: Statistics, const CUT_OUT: bool, N: StackEntry> {
    state: EntryState<Self>,
    /// The statistics of this entry's runs.
    pub statistics: S,
    nested: RuntimeStack<N>,
}

// SAFETY: as `SimpleRuntimeEntry`.
unsafe impl<S: Statistics, const CUT_OUT: bool, N: StackEntry> Sync
    for NestedRuntimeEntry<S, CUT_OUT, N>
{
}

impl<S: Statistics, const CUT_OUT: bool, N: StackEntry> NestedRuntimeEntry<S, CUT_OUT, N> {
    /// An unused entry over `statistics`.
    pub const fn new(statistics: S) -> Self {
        Self { state: EntryState::new(), statistics, nested: RuntimeStack::new() }
    }

    /// The stack of nested entries.
    pub fn nested(&self) -> &RuntimeStack<N> {
        &self.nested
    }

    /// Push `entry` on the nested stack.
    pub fn push_entry(&self, entry: &'static N, now: u32) {
        self.nested.push_entry(entry, now);
    }

    /// Pop `entry` from the nested stack, if it is on top.
    pub fn pop_entry(&self, entry: &'static N, now: u32) {
        self.nested.pop_entry(entry, now);
    }
}

impl<S: Statistics + 'static, const CUT_OUT: bool, N: StackEntry> StackEntry
    for NestedRuntimeEntry<S, CUT_OUT, N>
{
    fn is_used(&self) -> bool {
        self.state.used.get()
    }

    fn push(&self, now: u32, prev: Option<&'static Self>) {
        self.nested.resume(now);
        self.state.push(now, prev);
    }

    fn pop(&self, now: u32) -> Option<&'static Self> {
        self.nested.suspend(now);
        let (start, runtime, suspended, prev) = self.state.pop(now);
        self.statistics.add_run(start, runtime, suspended);
        if let Some(prev) = prev {
            prev.add_suspended_time(suspended.wrapping_add(if CUT_OUT { runtime } else { 0 }));
        }
        prev
    }

    fn add_suspended_time(&self, suspended_time: u32) {
        self.nested.add_suspended_time(suspended_time);
        self.state.suspended_time.set(self.state.suspended_time.get().wrapping_add(suspended_time));
    }
}

/// A stack of entries: the port of `RuntimeStack`.
pub struct RuntimeStack<E: StackEntry> {
    top: Cell<Option<&'static E>>,
    suspend_timestamp: Cell<u32>,
}

// SAFETY: as the entries.
unsafe impl<E: StackEntry> Sync for RuntimeStack<E> {}

impl<E: StackEntry> RuntimeStack<E> {
    /// An empty stack.
    pub const fn new() -> Self {
        Self { top: Cell::new(None), suspend_timestamp: Cell::new(0) }
    }

    /// The entry on top.
    pub fn top_entry(&self) -> Option<&'static E> {
        self.top.get()
    }

    /// Push `entry` at `now`, unless it is on a stack already.
    pub fn push_entry(&self, entry: &'static E, now: u32) {
        if !entry.is_used() {
            entry.push(now, self.top.get());
            self.top.set(Some(entry));
        }
    }

    /// Pop the top entry at `now`.
    pub fn pop_top(&self, now: u32) {
        if let Some(top) = self.top.get() {
            self.top.set(top.pop(now));
        }
    }

    /// Pop `entry` at `now`, if it is on top.
    pub fn pop_entry(&self, entry: &'static E, now: u32) {
        if let Some(top) = self.top.get()
            && core::ptr::eq(top, entry)
        {
            self.top.set(entry.pop(now));
        }
    }

    /// The stack stops running at `now`.
    pub fn suspend(&self, now: u32) {
        self.suspend_timestamp.set(now);
    }

    /// The stack runs again at `now`: the pause is charged to the top entry.
    pub fn resume(&self, now: u32) {
        self.add_suspended_time(now.wrapping_sub(self.suspend_timestamp.get()));
    }

    /// Charge `suspended_time` to the top entry.
    pub fn add_suspended_time(&self, suspended_time: u32) {
        if let Some(top) = self.top.get() {
            top.add_suspended_time(suspended_time);
        }
    }
}

impl<E: StackEntry> Default for RuntimeStack<E> {
    fn default() -> Self {
        Self::new()
    }
}

// Ported from runtime/test/src/RuntimeStackEntryTest.cpp, SimpleRuntimeEntryTest.cpp,
// NestedRuntimeEntryTest.cpp and RuntimeStackTest.cpp.
#[cfg(test)]
pub(crate) mod tests {
    extern crate std;

    use std::boxed::Box;
    use std::cell::RefCell;
    use std::vec::Vec;

    use super::*;

    /// Records every `add_run`: the port of the mocked `addRun`.
    #[derive(Default)]
    pub(crate) struct Recorder {
        pub(crate) runs: RefCell<Vec<(u32, u32, u32)>>,
        pub(crate) resets: RefCell<u32>,
    }
    // SAFETY: single-threaded test object.
    unsafe impl Sync for Recorder {}

    impl Recorder {
        pub(crate) fn take(&self) -> Vec<(u32, u32, u32)> {
            core::mem::take(&mut *self.runs.borrow_mut())
        }
    }

    impl Statistics for Recorder {
        fn add_run(&self, start_timestamp: u32, runtime: u32, suspended_time: u32) {
            self.runs.borrow_mut().push((start_timestamp, runtime, suspended_time));
        }
        fn reset(&self) {
            *self.resets.borrow_mut() += 1;
        }
        fn copy_from(&self, other: &Self) {
            *self.runs.borrow_mut() = other.runs.borrow().clone();
        }
    }

    type CutOutEntry = SimpleRuntimeEntry<Recorder, true>;
    type PlainEntry = SimpleRuntimeEntry<Recorder, false>;

    fn entry<E>(make: fn() -> E) -> &'static E {
        Box::leak(Box::new(make()))
    }

    #[test]
    fn cut_out_entry() {
        let entry1 = entry(|| CutOutEntry::new(Recorder::default()));
        let entry2 = entry(|| CutOutEntry::new(Recorder::default()));
        let entry3 = entry(|| CutOutEntry::new(Recorder::default()));
        assert!(!entry1.is_used());
        entry1.push(100, None);
        assert!(entry1.is_used());
        assert!(!entry2.is_used());
        entry2.push(200, Some(entry1));
        assert!(entry2.is_used());
        assert!(!entry3.is_used());
        entry3.push(400, Some(entry2));
        assert!(entry3.is_used());
        entry3.pop(600);
        assert!(!entry3.is_used());
        assert_eq!(entry3.statistics.take(), [(400, 200, 0)]);
        entry3.push(750, Some(entry2));
        entry3.pop(1000);
        assert_eq!(entry3.statistics.take(), [(750, 250, 0)]);
        entry2.pop(1300);
        assert!(!entry2.is_used());
        assert_eq!(entry2.statistics.take(), [(200, 650, 450)]);
        entry1.pop(1450);
        assert!(!entry1.is_used());
        assert_eq!(entry1.statistics.take(), [(100, 250, 1100)]);
    }

    #[test]
    fn non_cut_out_entry() {
        let entry1 = entry(|| PlainEntry::new(Recorder::default()));
        let entry2 = entry(|| PlainEntry::new(Recorder::default()));
        let entry3 = entry(|| PlainEntry::new(Recorder::default()));
        entry1.push(100, None);
        // A second push of a used entry is ignored.
        entry1.push(500, None);
        entry2.push(200, Some(entry1));
        entry3.push(400, Some(entry2));
        entry3.pop(600);
        assert_eq!(entry3.statistics.take(), [(400, 200, 0)]);
        entry3.push(750, Some(entry2));
        entry3.pop(1000);
        assert_eq!(entry3.statistics.take(), [(750, 250, 0)]);
        entry2.pop(1300);
        assert_eq!(entry2.statistics.take(), [(200, 1100, 0)]);
        entry1.pop(1450);
        assert_eq!(entry1.statistics.take(), [(100, 1350, 0)]);
    }

    #[test]
    fn simple_runtime_entry() {
        let entry1 = entry(|| CutOutEntry::new(Recorder::default()));
        let entry2 = entry(|| CutOutEntry::new(Recorder::default()));
        entry1.push(50, None);
        entry2.push(100, Some(entry1));
        entry2.pop(300);
        assert_eq!(entry2.statistics.take(), [(100, 200, 0)]);
        entry2.push(380, Some(entry1));
        entry2.pop(400);
        assert_eq!(entry2.statistics.take(), [(380, 20, 0)]);
        entry1.pop(490);
        assert_eq!(entry1.statistics.take(), [(50, 220, 220)]);
    }

    type NestedTestEntry = SimpleRuntimeEntry<Recorder, false>;
    type TestEntry = NestedRuntimeEntry<Recorder, true, NestedTestEntry>;

    #[test]
    fn nested_runtime_entry() {
        let entry1 = entry(|| TestEntry::new(Recorder::default()));
        let entry2 = entry(|| TestEntry::new(Recorder::default()));
        let nested1 = entry(|| NestedTestEntry::new(Recorder::default()));
        let nested2 = entry(|| NestedTestEntry::new(Recorder::default()));
        entry1.push(50, None);
        entry2.push(100, Some(entry1));
        entry2.pop(300);
        assert_eq!(entry2.statistics.take(), [(100, 200, 0)]);
        entry1.push_entry(nested1, 400);
        entry1.push_entry(nested2, 450);
        entry1.pop(500);
        assert_eq!(entry1.statistics.take(), [(50, 250, 200)]);
        entry1.push(750, None);
        entry1.pop_entry(nested2, 800);
        assert_eq!(nested2.statistics.take(), [(450, 100, 250)]);
        entry1.pop_entry(nested1, 830);
        assert_eq!(nested1.statistics.take(), [(400, 180, 250)]);
        assert!(entry1.nested().top_entry().is_none());
    }

    #[test]
    fn stack_push_pop() {
        let entry1 = entry(|| CutOutEntry::new(Recorder::default()));
        let entry2 = entry(|| CutOutEntry::new(Recorder::default()));
        let entry3 = entry(|| CutOutEntry::new(Recorder::default()));
        let cut = RuntimeStack::<CutOutEntry>::new();
        assert!(cut.top_entry().is_none());
        cut.push_entry(entry1, 100);
        assert!(core::ptr::eq(cut.top_entry().unwrap(), entry1));
        cut.push_entry(entry2, 200);
        assert!(core::ptr::eq(cut.top_entry().unwrap(), entry2));
        // Not on top: ignored.
        cut.pop_entry(entry1, 1000);
        assert!(core::ptr::eq(cut.top_entry().unwrap(), entry2));
        // Already used: ignored.
        cut.push_entry(entry2, 300);
        assert!(core::ptr::eq(cut.top_entry().unwrap(), entry2));
        cut.push_entry(entry3, 500);
        cut.pop_entry(entry3, 900);
        assert_eq!(entry3.statistics.take(), [(500, 400, 0)]);
        assert!(core::ptr::eq(cut.top_entry().unwrap(), entry2));
        cut.push_entry(entry3, 1000);
        cut.pop_entry(entry3, 1100);
        assert_eq!(entry3.statistics.take(), [(1000, 100, 0)]);
        cut.pop_entry(entry2, 1300);
        assert_eq!(entry2.statistics.take(), [(200, 600, 500)]);
        assert!(core::ptr::eq(cut.top_entry().unwrap(), entry1));
        cut.add_suspended_time(30);
        cut.pop_entry(entry1, 1500);
        assert_eq!(entry1.statistics.take(), [(100, 270, 1130)]);
        assert!(cut.top_entry().is_none());
    }

    #[test]
    fn stack_pop_top_entry() {
        let entry1 = entry(|| CutOutEntry::new(Recorder::default()));
        let cut = RuntimeStack::<CutOutEntry>::new();
        cut.push_entry(entry1, 100);
        cut.pop_top(300);
        assert_eq!(entry1.statistics.take(), [(100, 200, 0)]);
        cut.pop_top(300);
        assert!(entry1.statistics.take().is_empty());
    }
}
