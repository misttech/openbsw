// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Port of Eclipse OpenBSW's `libs/bsw/timer`: a sorted intrusive list of timeouts over a
//! wrapping 32-bit clock, driven by whoever owns the hardware alarm.
//!
//! A [`Timer`] keeps [`TimeoutNode`]s ordered by expiry. The owner calls
//! [`Timer::process_next_timeout`] until it returns `false`, then arms its alarm with
//! [`Timer::get_next_delta`]. A timeout is any `'static` object that embeds a
//! [`TimerLink`] and implements [`TimeoutNode::expired`], as the C++ `Timeout` subclasses
//! do. Objects are `'static` because the list links them by reference for as long as they
//! are set, like `etl::intrusive_forward_list` does by pointer.
//!
//! The list is protected by a [`Lock`], the RAII critical section of the platform.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

use core::cell::Cell;
use core::marker::PhantomData;

/// A scoped lock: taken by [`Lock::lock`], released on drop. The async platform binds it
/// to its critical section.
pub trait Lock {
    /// Take the lock.
    fn lock() -> Self;
}

/// A lock that does nothing, for single-threaded hosts and tests.
pub struct NoLock;

impl Lock for NoLock {
    fn lock() -> Self {
        NoLock
    }
}

/// A timeout in a list, with its link alongside: walking the list reads the links
/// directly, as the C++ list reaches the `Timeout` base's fields, instead of calling
/// [`TimeoutNode::link`] through the trait object for every node it passes.
#[derive(Clone, Copy)]
struct Entry {
    node: &'static dyn TimeoutNode,
    link: &'static TimerLink,
}

impl Entry {
    fn new(node: &'static dyn TimeoutNode) -> Self {
        Self { node, link: node.link() }
    }
}

/// The link and timing fields a timeout embeds: the port of `timer::Timeout`'s data.
pub struct TimerLink {
    next: Cell<Option<Entry>>,
    linked: Cell<bool>,
    /// The time when the timeout expires.
    time: Cell<u32>,
    /// The period of a cyclic timeout, 0 for a single shot.
    cycle_time: Cell<u32>,
}

// SAFETY: every access to the cells happens inside the owning `Timer`'s `Lock`, or on a
// node that is not in any list, so the sharing is serialized by the platform's critical
// section, as the C++ list's is.
unsafe impl Sync for TimerLink {}

impl TimerLink {
    /// An unlinked link.
    pub const fn new() -> Self {
        Self {
            next: Cell::new(None),
            linked: Cell::new(false),
            time: Cell::new(0),
            cycle_time: Cell::new(0),
        }
    }

    /// Whether the timeout is in a timer's list.
    pub fn is_linked(&self) -> bool {
        self.linked.get()
    }

    /// The time when the timeout expires.
    pub fn time(&self) -> u32 {
        self.time.get()
    }

    /// The period of a cyclic timeout, 0 for a single shot.
    pub fn cycle_time(&self) -> u32 {
        self.cycle_time.get()
    }
}

impl Default for TimerLink {
    fn default() -> Self {
        Self::new()
    }
}

/// A timeout: the port of `timer::Timeout`.
pub trait TimeoutNode: Sync {
    /// The link that puts this timeout into a timer's list.
    fn link(&self) -> &TimerLink;

    /// Called when the timeout expires.
    fn expired(&self);
}

/// A sorted list of timeouts: the port of `timer::Timer<LockGuard>`.
pub struct Timer<L: Lock> {
    first: Cell<Option<Entry>>,
    _lock: PhantomData<L>,
}

// SAFETY: the list head is read and written only inside `L`, except by `is_active`, which
// reads a node's own flag; the platform's critical section serializes the sharing.
unsafe impl<L: Lock> Sync for Timer<L> {}

fn same_node(a: &dyn TimeoutNode, b: &dyn TimeoutNode) -> bool {
    core::ptr::addr_eq(a, b)
}

impl<L: Lock> Timer<L> {
    /// An empty timer.
    pub const fn new() -> Self {
        Self { first: Cell::new(None), _lock: PhantomData }
    }

    /// Process the next elapsed timeout, if any.
    ///
    /// Returns `true` if a timeout expired exactly at `now`, so the caller should process
    /// the next one; `false` when nothing was due or the processed timeout was overdue.
    pub fn process_next_timeout(&self, now: u32) -> bool {
        let (timeout, diff_timeout) = {
            let _lock = L::lock();
            match self.first.get() {
                Some(first) => {
                    let diff_timeout = diff(first.link.time(), now);
                    if diff_timeout <= 0 {
                        self.first.set(first.link.next.take());
                        first.link.linked.set(false);
                        (first, diff_timeout)
                    } else {
                        return false;
                    }
                }
                None => return false,
            }
        };
        self.reschedule_cyclic_timeout(timeout, now);
        timeout.node.expired();
        diff_timeout == 0
    }

    /// The delta to the next timeout, `Some(0)` if one is already due, `None` if the list
    /// is empty. `now` must be the current time, not reused from `process_next_timeout`.
    pub fn get_next_delta(&self, now: u32) -> Option<u32> {
        let _lock = L::lock();
        let first = self.first.get()?;
        if diff(first.link.time(), now) < 0 {
            Some(0)
        } else {
            Some(first.link.time().wrapping_sub(now))
        }
    }

    /// Whether a timeout is set.
    pub fn is_active(&self, timeout: &dyn TimeoutNode) -> bool {
        timeout.link().is_linked()
    }

    /// Set a single-shot timeout `delay` after `now`. Returns `true` if it is now the first
    /// to expire, so the alarm must be rearmed.
    pub fn set(&self, timeout: &'static dyn TimeoutNode, delay: u32, now: u32) -> bool {
        self.add_timeout(Entry::new(timeout), delay.wrapping_add(now), 0, now)
    }

    /// Set a cyclic timeout with `period`. Returns `true` if it is now the first to expire.
    pub fn set_cyclic(&self, timeout: &'static dyn TimeoutNode, period: u32, now: u32) -> bool {
        self.add_timeout(Entry::new(timeout), period.wrapping_add(now), period, now)
    }

    /// Cancel a timeout; nothing happens if it is not set.
    pub fn cancel(&self, timeout: &dyn TimeoutNode) {
        if timeout.link().is_linked() {
            let _lock = L::lock();
            self.erase(timeout);
        }
    }

    fn reschedule_cyclic_timeout(&self, timeout: Entry, now: u32) {
        let link = timeout.link;
        if link.cycle_time() > 0 {
            self.add_timeout(
                timeout,
                link.cycle_time().wrapping_add(link.time()),
                link.cycle_time(),
                now,
            );
        }
    }

    /// Insert after every timeout that expires at the same time or earlier.
    fn add_timeout(
        &self,
        timeout: Entry,
        absolute_timeout: u32,
        cycle_time: u32,
        now: u32,
    ) -> bool {
        let link = timeout.link;
        link.time.set(absolute_timeout);
        link.cycle_time.set(cycle_time);
        let _lock = L::lock();
        let timeout_diff = diff(link.time(), now);
        let mut prev: Option<Entry> = None;
        let mut current = self.first.get();
        while let Some(entry) = current {
            if diff(entry.link.time(), now) > timeout_diff {
                break;
            }
            prev = Some(entry);
            current = entry.link.next.get();
        }
        link.next.set(current);
        link.linked.set(true);
        match prev {
            None => {
                self.first.set(Some(timeout));
                true
            }
            Some(prev) => {
                prev.link.next.set(Some(timeout));
                false
            }
        }
    }

    fn erase(&self, timeout: &dyn TimeoutNode) {
        let mut prev: Option<Entry> = None;
        let mut current = self.first.get();
        while let Some(entry) = current {
            if same_node(entry.node, timeout) {
                let next = entry.link.next.take();
                entry.link.linked.set(false);
                match prev {
                    None => self.first.set(next),
                    Some(prev) => prev.link.next.set(next),
                }
                return;
            }
            prev = Some(entry);
            current = entry.link.next.get();
        }
    }
}

impl<L: Lock> Default for Timer<L> {
    fn default() -> Self {
        Self::new()
    }
}

/// The signed distance from `b` to `a` on the wrapping clock.
fn diff(a: u32, b: u32) -> i32 {
    a.wrapping_sub(b) as i32
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use std::boxed::Box;
    use std::cell::RefCell;
    use std::vec::Vec;

    std::thread_local! {
        static LOCK_COUNT: Cell<u32> = const { Cell::new(0) };
        static LOCK_CALLS: Cell<u32> = const { Cell::new(0) };
    }

    /// Counts locks and checks that none nests, as `LockPolicyMock` does.
    struct TestLock;

    impl Lock for TestLock {
        fn lock() -> Self {
            LOCK_COUNT.with(|count| {
                assert_eq!(count.get(), 0, "cyclic lock discovered");
                count.set(count.get() + 1);
            });
            LOCK_CALLS.with(|calls| calls.set(calls.get() + 1));
            TestLock
        }
    }

    impl Drop for TestLock {
        fn drop(&mut self) {
            LOCK_COUNT.with(|count| count.set(count.get() - 1));
        }
    }

    type TestTimer = Timer<TestLock>;

    /// What a timeout does when it expires, the port of the gmock actions.
    #[derive(Clone, Copy)]
    enum Action {
        Nothing,
        Cancel,
        RescheduleOneShot(u32),
        RescheduleCyclic(u32),
    }

    struct TestTimeout {
        link: TimerLink,
        expired_count: Cell<u32>,
        action: Cell<Action>,
        timer: Cell<Option<&'static TestTimer>>,
        now: Cell<u32>,
    }

    // SAFETY: the tests are single-threaded per timeout object.
    unsafe impl Sync for TestTimeout {}

    impl TimeoutNode for TestTimeout {
        fn link(&self) -> &TimerLink {
            &self.link
        }

        fn expired(&self) {
            self.expired_count.set(self.expired_count.get() + 1);
            let (Some(timer), action) = (self.timer.get(), self.action.replace(Action::Nothing))
            else {
                return;
            };
            let this: &'static TestTimeout = self.as_static();
            match action {
                Action::Nothing => {}
                Action::Cancel => timer.cancel(this),
                Action::RescheduleOneShot(time) => {
                    timer.cancel(this);
                    timer.set(this, time, self.now.get());
                }
                Action::RescheduleCyclic(time) => {
                    timer.cancel(this);
                    timer.set_cyclic(this, time, self.now.get());
                }
            }
        }
    }

    impl TestTimeout {
        fn as_static(&self) -> &'static TestTimeout {
            // SAFETY: test timeouts are leaked boxes, so they live for the program.
            unsafe { &*(self as *const TestTimeout) }
        }

        fn take_expired(&self) -> u32 {
            self.expired_count.replace(0)
        }
    }

    fn timeout() -> &'static TestTimeout {
        Box::leak(Box::new(TestTimeout {
            link: TimerLink::new(),
            expired_count: Cell::new(0),
            action: Cell::new(Action::Nothing),
            timer: Cell::new(None),
            now: Cell::new(0),
        }))
    }

    fn timer() -> &'static TestTimer {
        Box::leak(Box::new(TestTimer::new()))
    }

    struct Fixture {
        timer: &'static TestTimer,
        t1: &'static TestTimeout,
        t2: &'static TestTimeout,
        t3: &'static TestTimeout,
        now: u32,
        alarms: RefCell<Vec<u32>>,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                timer: timer(),
                t1: timeout(),
                t2: timeout(),
                t3: timeout(),
                now: 0,
                alarms: RefCell::new(Vec::new()),
            }
        }

        /// `update`: process everything due, then record the alarm that would be set.
        fn update(&self, timer: &TestTimer, now: u32) {
            while timer.process_next_timeout(now) {}
            if let Some(delta) = timer.get_next_delta(now) {
                self.alarms.borrow_mut().push(delta);
            }
        }

        fn update_and_expect_trigger(&self, timer: &TestTimer, now: u32) {
            while timer.process_next_timeout(now) {}
            assert_eq!(timer.get_next_delta(now), Some(0));
        }

        fn set_and_expect_trigger(
            &self,
            timer: &TestTimer,
            timeout: &'static TestTimeout,
            time: u32,
        ) {
            assert!(timer.set(timeout, time, self.now));
            assert!(timer.is_active(timeout));
        }

        fn set_cyclic_and_expect_trigger(
            &self,
            timer: &TestTimer,
            timeout: &'static TestTimeout,
            time: u32,
        ) {
            assert!(timer.set_cyclic(timeout, time, self.now));
        }

        fn take_alarms(&self) -> Vec<u32> {
            core::mem::take(&mut *self.alarms.borrow_mut())
        }

        fn arm(&self, timeout: &'static TestTimeout, action: Action) {
            timeout.timer.set(Some(self.timer));
            timeout.now.set(self.now);
            timeout.action.set(action);
        }
    }

    // Ported from timer/test/src/TimerTest.cpp, one test per case.
    #[test]
    fn can_handle_a_single_timeout() {
        let mut f = Fixture::new();
        f.set_and_expect_trigger(f.timer, f.t1, 100);
        f.update(f.timer, f.now);
        f.now = 100;
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        f.now = 200;
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 0);
        assert!(f.take_alarms().contains(&100));
    }

    #[test]
    fn can_handle_single_cyclic_timeout() {
        let mut f = Fixture::new();
        f.set_cyclic_and_expect_trigger(f.timer, f.t1, 100);
        f.update(f.timer, f.now);
        f.now = 100;
        f.update(f.timer, f.now);
        f.now = 200;
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 2);
        assert!(f.take_alarms().iter().all(|&alarm| alarm == 100));
    }

    #[test]
    fn multiple_timeouts_with_different_expiration_registered_at_the_same_time_ordered() {
        let mut f = Fixture::new();
        f.set_and_expect_trigger(f.timer, f.t1, 100);
        f.timer.set(f.t2, 200, f.now);
        f.timer.set(f.t3, 300, f.now);
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [100]);
        f.now = 100;
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        assert_eq!(f.take_alarms(), [100]);
        // The alarm is reset to the first element although now is still 100.
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [100]);
        f.now = 200;
        f.update(f.timer, f.now);
        assert_eq!(f.t2.take_expired(), 1);
        assert_eq!(f.take_alarms(), [100]);
        f.now = 300;
        f.update(f.timer, f.now);
        assert_eq!(f.t3.take_expired(), 1);
        f.now = 400;
        f.update(f.timer, f.now);
        assert_eq!((f.t1.take_expired(), f.t2.take_expired(), f.t3.take_expired()), (0, 0, 0));
    }

    #[test]
    fn multiple_timeouts_with_different_expiration_registered_unordered_3_2_1() {
        let mut f = Fixture::new();
        f.set_and_expect_trigger(f.timer, f.t1, 300);
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [300]);
        f.set_and_expect_trigger(f.timer, f.t2, 200);
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [200]);
        f.set_and_expect_trigger(f.timer, f.t3, 100);
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [100]);
        f.now = 100;
        f.update(f.timer, f.now);
        assert_eq!(f.t3.take_expired(), 1);
        assert_eq!(f.take_alarms(), [100]);
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [100]);
        f.now = 200;
        f.update(f.timer, f.now);
        assert_eq!(f.t2.take_expired(), 1);
        assert_eq!(f.take_alarms(), [100]);
        f.now = 300;
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
    }

    #[test]
    fn multiple_timeouts_with_the_same_expiration_time_at_the_same_time() {
        let mut f = Fixture::new();
        f.set_and_expect_trigger(f.timer, f.t1, 300);
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [300]);
        f.timer.set(f.t2, 300, f.now);
        f.timer.set(f.t3, 300, f.now);
        f.now = 100;
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [200]);
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [200]);
        f.now = 200;
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [100]);
        f.now = 300;
        f.update(f.timer, f.now);
        assert_eq!((f.t1.take_expired(), f.t2.take_expired(), f.t3.take_expired()), (1, 1, 1));
        f.now = 400;
        f.update(f.timer, f.now);
        assert_eq!((f.t1.take_expired(), f.t2.take_expired(), f.t3.take_expired()), (0, 0, 0));
    }

    #[test]
    fn multiple_timeouts_with_the_same_expiration_time_at_different_times() {
        let mut f = Fixture::new();
        f.set_and_expect_trigger(f.timer, f.t1, 300);
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [300]);
        f.now = 100;
        f.timer.set(f.t2, 200, f.now);
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [200]);
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [200]);
        f.now = 200;
        f.timer.set(f.t3, 100, f.now);
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [100]);
        f.now = 300;
        f.update(f.timer, f.now);
        assert_eq!((f.t1.take_expired(), f.t2.take_expired(), f.t3.take_expired()), (1, 1, 1));
        f.now = 400;
        f.update(f.timer, f.now);
        assert_eq!((f.t1.take_expired(), f.t2.take_expired(), f.t3.take_expired()), (0, 0, 0));
    }

    #[test]
    fn the_same_timeout_is_added_multiple_times() {
        let mut f = Fixture::new();
        f.set_and_expect_trigger(f.timer, f.t1, 100);
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [100]);
        // The C++ Timer::set() cannot add a linked timeout twice; its callers check
        // isActive() first. The Rust port keeps that contract at the TaskContext level,
        // so here the second set re-links with the new values.
        f.timer.cancel(f.t1);
        f.timer.set(f.t1, 100, f.now);
        f.now = 100;
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
    }

    #[test]
    fn can_handle_canceled_timeout_from_different_task() {
        let mut f = Fixture::new();
        f.set_and_expect_trigger(f.timer, f.t1, 100);
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [100]);
        f.timer.cancel(f.t1);
        f.update(f.timer, f.now);
        assert!(f.take_alarms().is_empty());
        f.now = 100;
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 0);
    }

    #[test]
    fn lock_is_invoked_for_adding_processing_and_iterating() {
        let mut f = Fixture::new();
        LOCK_CALLS.with(|calls| calls.set(0));
        f.set_and_expect_trigger(f.timer, f.t1, 100);
        assert!(LOCK_CALLS.with(|calls| calls.get()) >= 1);
        LOCK_CALLS.with(|calls| calls.set(0));
        f.update(f.timer, f.now);
        assert!(LOCK_CALLS.with(|calls| calls.get()) >= 1);
        f.now = 100;
        LOCK_CALLS.with(|calls| calls.set(0));
        f.update(f.timer, f.now);
        assert!(LOCK_CALLS.with(|calls| calls.get()) >= 1);
        assert_eq!(f.t1.take_expired(), 1);
        assert_eq!(LOCK_COUNT.with(|count| count.get()), 0);
    }

    #[test]
    fn handles_timeouts_which_overflow_32bit_time() {
        let mut f = Fixture::new();
        f.now = 0xFFFF_FF92; // 110 before overflow
        f.set_and_expect_trigger(f.timer, f.t1, 100);
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [100]);
        f.timer.set(f.t2, 200, f.now);
        f.now = 0xFFFF_FFF6; // 10 before overflow
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        assert_eq!(f.take_alarms(), [100]);
        f.now = 50; // 50 after overflow
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [40]);
        f.now = 90;
        f.update(f.timer, f.now);
        assert_eq!(f.t2.take_expired(), 1);
        f.now = 190;
        f.update(f.timer, f.now);
        f.now = 200;
        f.update(f.timer, f.now);
        assert_eq!((f.t1.take_expired(), f.t2.take_expired()), (0, 0));
    }

    #[test]
    fn handles_timeouts_which_overflow_32bit_time_unordered_registration_3_2_1() {
        let mut f = Fixture::new();
        f.now = 0xFFFF_FF92;
        f.set_and_expect_trigger(f.timer, f.t1, 300);
        f.update(f.timer, f.now);
        f.set_and_expect_trigger(f.timer, f.t2, 200);
        f.update(f.timer, f.now);
        f.set_and_expect_trigger(f.timer, f.t3, 100);
        f.update(f.timer, f.now);
        f.now = 0xFFFF_FFF6;
        f.update(f.timer, f.now);
        f.now = 90;
        f.update(f.timer, f.now);
        f.now = 190;
        f.update(f.timer, f.now);
        assert_eq!((f.t1.take_expired(), f.t2.take_expired(), f.t3.take_expired()), (1, 1, 1));
        let alarms = f.take_alarms();
        assert_eq!(alarms.iter().filter(|&&a| a == 300).count(), 1);
        assert_eq!(alarms.iter().filter(|&&a| a == 200).count(), 1);
        assert_eq!(alarms.iter().filter(|&&a| a == 100).count(), 3);
        f.now = 200;
        f.update(f.timer, f.now);
        assert_eq!((f.t1.take_expired(), f.t2.take_expired(), f.t3.take_expired()), (0, 0, 0));
    }

    #[test]
    fn handles_timeouts_which_overflow_32bit_time_unordered_registration_2_3_1() {
        let mut f = Fixture::new();
        f.now = 0xFFFF_FF92;
        f.set_and_expect_trigger(f.timer, f.t1, 200);
        f.update(f.timer, f.now);
        f.timer.set(f.t2, 300, f.now);
        f.set_and_expect_trigger(f.timer, f.t3, 100);
        f.update(f.timer, f.now);
        f.now = 0xFFFF_FFF6;
        f.update(f.timer, f.now);
        f.now = 90;
        f.update(f.timer, f.now);
        f.now = 190;
        f.update(f.timer, f.now);
        assert_eq!((f.t1.take_expired(), f.t2.take_expired(), f.t3.take_expired()), (1, 1, 1));
        let alarms = f.take_alarms();
        assert_eq!(alarms.iter().filter(|&&a| a == 200).count(), 1);
        assert_eq!(alarms.iter().filter(|&&a| a == 100).count(), 3);
        f.now = 200;
        f.update(f.timer, f.now);
        assert_eq!((f.t1.take_expired(), f.t2.take_expired(), f.t3.take_expired()), (0, 0, 0));
    }

    #[test]
    fn handles_timeouts_which_overflow_32bit_time_unordered_registration_1_3_2() {
        let mut f = Fixture::new();
        f.now = 0xFFFF_FF92;
        f.set_and_expect_trigger(f.timer, f.t1, 100);
        f.update(f.timer, f.now);
        f.timer.set(f.t2, 300, f.now);
        f.timer.set(f.t3, 200, f.now);
        f.now = 0xFFFF_FFF6;
        f.update(f.timer, f.now);
        f.now = 90;
        f.update(f.timer, f.now);
        f.now = 190;
        f.update(f.timer, f.now);
        assert_eq!((f.t1.take_expired(), f.t2.take_expired(), f.t3.take_expired()), (1, 1, 1));
        assert_eq!(f.take_alarms(), [100, 100, 100]);
        f.now = 200;
        f.update(f.timer, f.now);
        assert_eq!((f.t1.take_expired(), f.t2.take_expired(), f.t3.take_expired()), (0, 0, 0));
    }

    #[test]
    fn single_timeout_can_be_canceled() {
        let mut f = Fixture::new();
        f.now = 50;
        f.set_and_expect_trigger(f.timer, f.t1, 100);
        f.update(f.timer, f.now);
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [100, 100]);
        f.timer.cancel(f.t1);
        f.now = 100;
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 0);
    }

    #[test]
    fn next_timeout_can_be_canceled_and_alarm_is_reset() {
        let mut f = Fixture::new();
        f.set_and_expect_trigger(f.timer, f.t1, 100);
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [100]);
        f.timer.set(f.t2, 200, f.now);
        f.now = 50;
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [50]);
        f.timer.cancel(f.t1);
        f.now = 100;
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [100]);
        f.now = 200;
        f.update(f.timer, f.now);
        assert_eq!((f.t1.take_expired(), f.t2.take_expired()), (0, 1));
    }

    #[test]
    fn timeout_action_cancel_cancels_cyclic_timeout() {
        let mut f = Fixture::new();
        f.set_cyclic_and_expect_trigger(f.timer, f.t1, 100);
        f.update(f.timer, f.now);
        f.now = 100;
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        assert_eq!(f.take_alarms(), [100, 100]);
        f.now = 200;
        f.arm(f.t1, Action::Cancel);
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        f.now = 300;
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 0);
        assert!(f.take_alarms().is_empty());
    }

    #[test]
    fn timeout_action_reschedule_one_shot_timeout() {
        let mut f = Fixture::new();
        f.set_and_expect_trigger(f.timer, f.t1, 100);
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [100]);
        f.now = 100;
        f.arm(f.t1, Action::RescheduleOneShot(50));
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        assert_eq!(f.take_alarms(), [50]);
        f.now = 150;
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        f.now = 200;
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 0);
    }

    #[test]
    fn cyclic_timeout_reschedule_with_different_time() {
        let mut f = Fixture::new();
        f.set_cyclic_and_expect_trigger(f.timer, f.t1, 100);
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [100]);
        f.now = 100;
        f.arm(f.t1, Action::RescheduleCyclic(50));
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        assert_eq!(f.take_alarms(), [50]);
        f.now = 150;
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        assert_eq!(f.take_alarms(), [50]);
        f.now = 200;
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        assert_eq!(f.take_alarms(), [50]);
    }

    #[test]
    fn cyclic_timeout_can_be_canceled_with_cancel_function() {
        let mut f = Fixture::new();
        f.set_cyclic_and_expect_trigger(f.timer, f.t1, 100);
        f.update(f.timer, f.now);
        f.now = 100;
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        assert_eq!(f.take_alarms(), [100, 100]);
        f.timer.cancel(f.t1);
        f.now = 200;
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 0);
    }

    #[test]
    fn cancel_action_one_shot_timeout_does_nothing() {
        let mut f = Fixture::new();
        f.set_and_expect_trigger(f.timer, f.t1, 100);
        f.update(f.timer, f.now);
        f.now = 100;
        f.arm(f.t1, Action::Cancel);
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        f.now = 200;
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 0);
    }

    #[test]
    fn handles_blocking_task_and_cancel_of_an_unset_timeout() {
        let mut f = Fixture::new();
        f.set_and_expect_trigger(f.timer, f.t1, 10);
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [10]);
        f.now = 50;
        f.timer.cancel(f.t2);
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
    }

    #[test]
    fn can_handle_recovery_of_alarm_jitter_with_2_single_shots_timeouts() {
        let mut f = Fixture::new();
        f.set_and_expect_trigger(f.timer, f.t1, 100);
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [100]);
        f.timer.set(f.t2, 200, f.now);
        f.now = 110;
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        assert_eq!(f.take_alarms(), [90]);
        f.now = 205;
        f.update(f.timer, f.now);
        assert_eq!(f.t2.take_expired(), 1);
    }

    #[test]
    fn can_handle_recovery_of_alarm_jitter_with_1_cyclic_timeout() {
        let mut f = Fixture::new();
        f.set_cyclic_and_expect_trigger(f.timer, f.t1, 100);
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [100]);
        f.now = 110;
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        assert_eq!(f.take_alarms(), [90]);
        f.now = 205;
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        assert_eq!(f.take_alarms(), [95]);
    }

    #[test]
    fn can_not_handle_recovery_of_alarm_jitter_with_1_single_shot_timeout_reschedule() {
        let mut f = Fixture::new();
        f.set_and_expect_trigger(f.timer, f.t1, 100);
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [100]);
        f.now = 110;
        f.arm(f.t1, Action::RescheduleOneShot(100));
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        assert_eq!(f.take_alarms(), [100]);
        f.now = 215;
        f.arm(f.t1, Action::RescheduleOneShot(100));
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        assert_eq!(f.take_alarms(), [100]);
    }

    #[test]
    fn can_not_handle_recovery_of_alarm_jitter_with_1_cyclic_timeout_reschedule_same_time() {
        let mut f = Fixture::new();
        f.set_cyclic_and_expect_trigger(f.timer, f.t1, 100);
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [100]);
        f.now = 110;
        f.arm(f.t1, Action::RescheduleCyclic(100));
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        assert_eq!(f.take_alarms(), [100]);
        f.now = 215;
        f.arm(f.t1, Action::RescheduleCyclic(100));
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        assert_eq!(f.take_alarms(), [100]);
    }

    #[test]
    fn can_not_handle_recovery_of_alarm_jitter_with_1_cyclic_timeout_reschedule_different_time() {
        let mut f = Fixture::new();
        f.set_cyclic_and_expect_trigger(f.timer, f.t1, 100);
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [100]);
        f.now = 210;
        f.arm(f.t1, Action::RescheduleCyclic(200));
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        assert_eq!(f.take_alarms(), [200]);
        f.now = 415;
        f.arm(f.t1, Action::RescheduleCyclic(100));
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        assert_eq!(f.take_alarms(), [100]);
    }

    #[test]
    fn can_not_handle_recovery_of_alarm_jitter_with_reschedule_time_smaller_than_jitter() {
        let mut f = Fixture::new();
        f.set_cyclic_and_expect_trigger(f.timer, f.t1, 100);
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [100]);
        f.now = 110;
        f.arm(f.t1, Action::RescheduleCyclic(5));
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        assert_eq!(f.take_alarms(), [5]);
        f.now = 115;
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        assert_eq!(f.take_alarms(), [5]);
        f.now = 122;
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        assert_eq!(f.take_alarms(), [3]);
    }

    #[test]
    fn can_handle_recovery_of_alarm_jitter_with_1_cyclic_timeout_jitter_bigger_than_alarm_time() {
        let mut f = Fixture::new();
        f.set_cyclic_and_expect_trigger(f.timer, f.t1, 2);
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [2]);
        f.now = 5;
        f.update_and_expect_trigger(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        assert_eq!(f.take_alarms(), [1]);
    }

    #[test]
    fn two_timer_instances_with_single_shot_timeouts() {
        let mut f = Fixture::new();
        let timer1 = timer();
        let timer2 = timer();
        f.set_and_expect_trigger(timer1, f.t1, 100);
        f.update(timer1, f.now);
        f.set_and_expect_trigger(timer2, f.t2, 100);
        f.update(timer2, f.now);
        assert_eq!(f.take_alarms(), [100, 100]);
        f.now = 100;
        f.update(timer1, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        f.update(timer2, f.now);
        assert_eq!(f.t2.take_expired(), 1);
        f.now = 200;
        f.update(timer1, f.now);
        f.update(timer2, f.now);
        assert_eq!((f.t1.take_expired(), f.t2.take_expired()), (0, 0));
    }

    #[test]
    fn two_timer_instances_with_cyclic_timeouts() {
        let mut f = Fixture::new();
        let timer1 = timer();
        let timer2 = timer();
        f.set_cyclic_and_expect_trigger(timer1, f.t1, 100);
        f.update(timer1, f.now);
        f.set_cyclic_and_expect_trigger(timer2, f.t2, 100);
        f.update(timer2, f.now);
        f.now = 100;
        f.update(timer1, f.now);
        f.update(timer2, f.now);
        f.now = 200;
        f.update(timer1, f.now);
        f.update(timer2, f.now);
        assert_eq!((f.t1.take_expired(), f.t2.take_expired()), (2, 2));
        assert_eq!(f.take_alarms(), [100; 6]);
    }

    #[test]
    fn check_order_of_expired_callbacks_with_same_expiration_time() {
        let mut f = Fixture::new();
        std::thread_local! {
            static ORDER: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
        }
        struct Ordered {
            link: TimerLink,
            tag: u8,
        }
        // SAFETY: single-threaded test objects.
        unsafe impl Sync for Ordered {}
        impl TimeoutNode for Ordered {
            fn link(&self) -> &TimerLink {
                &self.link
            }
            fn expired(&self) {
                ORDER.with(|order| order.borrow_mut().push(self.tag));
            }
        }
        let o1: &'static Ordered = Box::leak(Box::new(Ordered { link: TimerLink::new(), tag: 1 }));
        let o2: &'static Ordered = Box::leak(Box::new(Ordered { link: TimerLink::new(), tag: 2 }));
        let o3: &'static Ordered = Box::leak(Box::new(Ordered { link: TimerLink::new(), tag: 3 }));
        assert!(f.timer.set(o1, 100, f.now));
        f.update(f.timer, f.now);
        f.timer.set(o2, 100, f.now);
        f.timer.set(o3, 100, f.now);
        f.now = 100;
        f.update(f.timer, f.now);
        assert_eq!(ORDER.with(|order| order.borrow().clone()), [1, 2, 3]);
        f.now = 400;
        f.update(f.timer, f.now);
        assert_eq!(ORDER.with(|order| order.borrow().len()), 3);
    }

    #[test]
    fn can_handle_recovery_of_cyclic_timeout_that_is_far_behind() {
        let mut f = Fixture::new();
        f.set_cyclic_and_expect_trigger(f.timer, f.t1, 10);
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [10]);
        f.now = 50; // 5 timeouts behind
        f.update_and_expect_trigger(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        f.update_and_expect_trigger(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        f.now = 60;
        f.update_and_expect_trigger(f.timer, f.now);
        f.update_and_expect_trigger(f.timer, f.now);
        f.update_and_expect_trigger(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 3);
        f.update(f.timer, f.now); // recovered
        assert_eq!(f.t1.take_expired(), 1);
        assert_eq!(f.take_alarms(), [10]);
        f.now = 70;
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        f.now = 80;
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        assert_eq!(f.take_alarms(), [10, 10]);
    }

    #[test]
    fn can_handle_recovery_of_alarm_jitter_with_1_cyclic_timeout_and_overflow() {
        let mut f = Fixture::new();
        f.now = 0xFFFF_FF92;
        f.set_cyclic_and_expect_trigger(f.timer, f.t1, 100);
        f.update(f.timer, f.now);
        assert_eq!(f.take_alarms(), [100]);
        f.now = 0; // 10 of jitter
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        assert_eq!(f.take_alarms(), [90]);
        f.now = 90;
        f.update(f.timer, f.now);
        assert_eq!(f.t1.take_expired(), 1);
        assert_eq!(f.take_alarms(), [100]);
    }

    #[test]
    fn can_handle_delays_between_processing_timeouts_and_getting_next_timeout_value() {
        let mut f = Fixture::new();
        f.set_cyclic_and_expect_trigger(f.timer, f.t1, 10);
        f.update(f.timer, f.now);
        f.now = 10;
        assert!(f.timer.process_next_timeout(f.now));
        assert_eq!(f.t1.take_expired(), 1);
        // Time passes between process_next_timeout and get_next_delta.
        f.now += 5;
        assert_eq!(f.timer.get_next_delta(f.now), Some(5));
    }

    #[test]
    fn is_active_and_cancel_of_an_unset_timeout() {
        let f = Fixture::new();
        assert!(!f.timer.is_active(f.t1));
        f.timer.cancel(f.t1);
        assert!(f.timer.set(f.t1, 10, 0));
        assert!(f.timer.is_active(f.t1));
        assert_eq!(f.t1.link().time(), 10);
        assert_eq!(f.t1.link().cycle_time(), 0);
        f.timer.cancel(f.t1);
        assert!(!f.timer.is_active(f.t1));
        assert_eq!(f.timer.get_next_delta(0), None);
    }
}
