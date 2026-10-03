// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The run time monitor, ported from `RuntimeMonitor.h`: fed by the context-switch and
//! interrupt hooks, it keeps one stack entry per task and per interrupt group.

use core::cell::Cell;
use core::marker::PhantomData;

use openbsw_timer::Lock;

use crate::container::StatisticsContainer;
use crate::stack::{NestedRuntimeEntry, RuntimeStack, SimpleRuntimeEntry};
use crate::statistics::Statistics;

/// A measured function inside a context: its time is not taken out of the context's.
pub type FunctionEntry<FS> = SimpleRuntimeEntry<FS, false>;

/// A task or interrupt group: its time is taken out of what it preempted, and it holds
/// the stack of its functions.
pub type ContextEntry<CS, FS> = NestedRuntimeEntry<CS, true, FunctionEntry<FS>>;

/// Tracks which context and function run, charging their run times to their entries.
///
/// `CS` and `FS` are the statistics of a context and of a function; `L` is the platform
/// lock, taken around every update as the C++ `::async::LockType` is; `ticks` is the
/// clock (`getSystemTicks32Bit`).
pub struct RuntimeMonitor<CS: Statistics + 'static, FS: Statistics + 'static, L: Lock> {
    context_stack: RuntimeStack<ContextEntry<CS, FS>>,
    task_statistics: &'static StatisticsContainer<ContextEntry<CS, FS>>,
    isr_group_statistics: &'static StatisticsContainer<ContextEntry<CS, FS>>,
    start_timestamp: Cell<u32>,
    last_enter_task_timestamp: Cell<u32>,
    ticks: &'static (dyn Fn() -> u32 + Sync),
    _lock: PhantomData<fn() -> L>,
}

// SAFETY: the cells change under `L`, the platform lock.
unsafe impl<CS: Statistics, FS: Statistics, L: Lock> Sync for RuntimeMonitor<CS, FS, L> {}

impl<CS: Statistics + 'static, FS: Statistics + 'static, L: Lock> RuntimeMonitor<CS, FS, L> {
    /// A monitor over the entries of the tasks and of the interrupt groups, reading the
    /// time from `ticks`.
    pub const fn new(
        task_statistics: &'static StatisticsContainer<ContextEntry<CS, FS>>,
        isr_group_statistics: &'static StatisticsContainer<ContextEntry<CS, FS>>,
        ticks: &'static (dyn Fn() -> u32 + Sync),
    ) -> Self {
        Self {
            context_stack: RuntimeStack::new(),
            task_statistics,
            isr_group_statistics,
            start_timestamp: Cell::new(0),
            last_enter_task_timestamp: Cell::new(0),
            ticks,
            _lock: PhantomData,
        }
    }

    /// The tasks' entries.
    pub fn task_statistics(&self) -> &'static StatisticsContainer<ContextEntry<CS, FS>> {
        self.task_statistics
    }

    /// The interrupt groups' entries.
    pub fn isr_group_statistics(&self) -> &'static StatisticsContainer<ContextEntry<CS, FS>> {
        self.isr_group_statistics
    }

    /// The time from the start (or last reset) to the last task switch.
    pub fn last_runtime(&self) -> u32 {
        let _lock = L::lock();
        self.last_enter_task_timestamp.get().wrapping_sub(self.start_timestamp.get())
    }

    /// Start measuring now.
    pub fn start(&self) {
        let now = (self.ticks)();
        self.start_timestamp.set(now);
        self.last_enter_task_timestamp.set(now);
    }

    /// Pop the running context.
    pub fn stop(&self) {
        let _lock = L::lock();
        self.context_stack.pop_top((self.ticks)());
    }

    /// Reset every entry and the measurement start; returns the time measured so far.
    pub fn reset(&self) -> u32 {
        let _lock = L::lock();
        let last_runtime =
            self.last_enter_task_timestamp.get().wrapping_sub(self.start_timestamp.get());
        self.start_timestamp.set(self.last_enter_task_timestamp.get());
        for entry in self.task_statistics.entries() {
            entry.statistics.reset();
        }
        for entry in self.isr_group_statistics.entries() {
            entry.statistics.reset();
        }
        last_runtime
    }

    /// Task `task_idx` was switched in.
    pub fn enter_task(&self, task_idx: usize) {
        let _lock = L::lock();
        let timestamp = (self.ticks)();
        self.last_enter_task_timestamp.set(timestamp);
        if let Some(entry) = self.task_statistics.entry(task_idx) {
            self.context_stack.push_entry(entry, timestamp);
        }
    }

    /// Task `task_idx` was switched out.
    pub fn leave_task(&self, task_idx: usize) {
        let _lock = L::lock();
        let timestamp = (self.ticks)();
        if let Some(entry) = self.task_statistics.entry(task_idx) {
            self.context_stack.pop_entry(entry, timestamp);
        }
    }

    /// An interrupt of group `isr_group_idx` began.
    pub fn enter_isr_group(&self, isr_group_idx: usize) {
        let _lock = L::lock();
        let timestamp = (self.ticks)();
        if let Some(entry) = self.isr_group_statistics.entry(isr_group_idx) {
            self.context_stack.push_entry(entry, timestamp);
        }
    }

    /// An interrupt of group `isr_group_idx` ended.
    pub fn leave_isr_group(&self, isr_group_idx: usize) {
        let _lock = L::lock();
        let timestamp = (self.ticks)();
        if let Some(entry) = self.isr_group_statistics.entry(isr_group_idx) {
            self.context_stack.pop_entry(entry, timestamp);
        }
    }

    /// `function_entry` started running on the current context.
    pub fn enter_function(&self, function_entry: &'static FunctionEntry<FS>) {
        let _lock = L::lock();
        if let Some(top) = self.context_stack.top_entry() {
            top.push_entry(function_entry, (self.ticks)());
        }
    }

    /// `function_entry` stopped running on the current context.
    pub fn leave_function(&self, function_entry: &'static FunctionEntry<FS>) {
        let _lock = L::lock();
        if let Some(top) = self.context_stack.top_entry() {
            top.pop_entry(function_entry, (self.ticks)());
        }
    }
}

// Ported from runtime/test/src/RuntimeMonitorTest.cpp.
#[cfg(test)]
mod tests {
    extern crate std;

    use core::sync::atomic::{AtomicU32, Ordering};
    use std::boxed::Box;

    use openbsw_timer::NoLock;

    use super::*;
    use crate::stack::tests::Recorder;

    type Entry = ContextEntry<Recorder, Recorder>;
    type Cut = RuntimeMonitor<Recorder, Recorder, NoLock>;

    fn entries<const N: usize>() -> &'static [Entry; N] {
        Box::leak(Box::new(core::array::from_fn(|_| Entry::new(Recorder::default()))))
    }

    fn task_name(idx: usize) -> Option<&'static [u8]> {
        [Some(&b"a"[..]), Some(b"b"), Some(b"c")].get(idx).copied().flatten()
    }

    #[test]
    fn all() {
        let clock: &'static AtomicU32 = Box::leak(Box::new(AtomicU32::new(0)));
        let set = |value: u32| clock.store(value, Ordering::Relaxed);
        let ticks = Box::leak(Box::new(move || clock.load(Ordering::Relaxed)))
            as &'static (dyn Fn() -> u32 + Sync);
        let task_entries = entries::<3>();
        let isr_entries = entries::<2>();
        let tasks: &'static StatisticsContainer<Entry> =
            Box::leak(Box::new(StatisticsContainer::new(task_entries, Some(&task_name))));
        let isrs: &'static StatisticsContainer<Entry> =
            Box::leak(Box::new(StatisticsContainer::new(isr_entries, Some(&task_name))));
        let functions: &'static [FunctionEntry<Recorder>; 2] =
            Box::leak(Box::new(core::array::from_fn(|_| FunctionEntry::new(Recorder::default()))));
        let cut = Cut::new(tasks, isrs, ticks);
        assert!(core::ptr::eq(cut.task_statistics(), tasks));
        assert!(core::ptr::eq(cut.isr_group_statistics(), isrs));
        set(1000);
        cut.start();
        // No context runs: functions are ignored.
        cut.enter_function(&functions[1]);
        cut.leave_function(&functions[1]);
        set(1010);
        cut.enter_task(0);
        set(1020);
        cut.enter_function(&functions[0]);
        set(1038);
        cut.leave_task(0);
        assert_eq!(task_entries[0].statistics.take(), [(1010, 28, 0)]);
        set(1040);
        cut.enter_task(1);
        set(1089);
        cut.leave_task(1);
        assert_eq!(task_entries[1].statistics.take(), [(1040, 49, 0)]);
        set(1090);
        cut.enter_task(0);
        assert_eq!(cut.reset(), 90);
        for entry in task_entries.iter().chain(isr_entries.iter()) {
            assert_eq!(*entry.statistics.resets.borrow(), 1);
        }
        assert_eq!(cut.last_runtime(), 0);
        set(1190);
        cut.enter_isr_group(0);
        set(1230);
        cut.enter_isr_group(1);
        set(1300);
        cut.leave_isr_group(1);
        assert_eq!(isr_entries[1].statistics.take(), [(1230, 70, 0)]);
        set(1450);
        cut.leave_isr_group(0);
        assert_eq!(isr_entries[0].statistics.take(), [(1190, 190, 70)]);
        set(1500);
        cut.enter_function(&functions[1]);
        set(1600);
        cut.leave_function(&functions[1]);
        assert_eq!(functions[1].statistics.take(), [(1500, 100, 0)]);
        set(1750);
        cut.leave_function(&functions[0]);
        assert_eq!(functions[0].statistics.take(), [(1020, 418, 312)]);
        set(1850);
        cut.stop();
        assert_eq!(task_entries[0].statistics.take(), [(1090, 500, 260)]);
        // Unknown indices are ignored, where the C++ indexes out of bounds.
        cut.enter_task(7);
        cut.leave_isr_group(7);
    }
}
