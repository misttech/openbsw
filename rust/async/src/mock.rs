// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! A scripted binding for host tests: the port of `async/mock`'s `TestContext`.
//!
//! [`MockAsync`] queues what the code under test executes and schedules, and runs it when
//! the test says so: [`run_runnables`](MockAsync::run_runnables) drains a context's
//! runnables (new ones queued meanwhile included), [`run_timeouts`](MockAsync::run_timeouts)
//! runs the timeouts due on a context at the mock's time, and
//! [`run_until_idle`](MockAsync::run_until_idle) emulates
//! the fixed-priority scheduler: after every runnable it goes back to the highest-priority
//! context with work.

extern crate std;

use std::sync::Mutex;
use std::vec::Vec;

use crate::binding::Async;
use crate::types::{ContextType, Runnable, TimeUnit, Timeout};

struct Scheduled {
    timeout: &'static Timeout,
    runnable: &'static dyn Runnable,
    context: ContextType,
    elapse_at: u64,
    period: u64,
}

struct State<const N: usize> {
    runnables: [Vec<&'static dyn Runnable>; N],
    timeouts: Vec<Scheduled>,
    now: u64,
    current: ContextType,
}

/// The scripted binding for `N` contexts.
pub struct MockAsync<const N: usize> {
    state: Mutex<State<N>>,
    names: [&'static [u8]; N],
}

fn same_timeout(a: &Timeout, b: &Timeout) -> bool {
    core::ptr::addr_eq(a, b)
}

fn same_runnable(a: &dyn Runnable, b: &dyn Runnable) -> bool {
    core::ptr::addr_eq(a, b)
}

impl<const N: usize> MockAsync<N> {
    /// A mock whose contexts are named `names`, at time 0.
    pub const fn new(names: [&'static [u8]; N]) -> Self {
        Self {
            state: Mutex::new(State {
                runnables: [const { Vec::new() }; N],
                timeouts: Vec::new(),
                now: 0,
                current: 0,
            }),
            names,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State<N>> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Set the time, in microseconds.
    pub fn set_now(&self, now_us: u64) {
        self.lock().now = now_us;
    }

    /// Advance the time by `delay_us`.
    pub fn elapse(&self, delay_us: u64) {
        self.lock().now += delay_us;
    }

    /// The time, in microseconds.
    pub fn now(&self) -> u64 {
        self.lock().now
    }

    /// Whether `context` has a runnable queued.
    pub fn has_runnable(&self, context: ContextType) -> bool {
        !self.lock().runnables[usize::from(context)].is_empty()
    }

    /// Whether a timeout is scheduled.
    pub fn is_scheduled(&self, timeout: &Timeout) -> bool {
        self.lock().timeouts.iter().any(|scheduled| same_timeout(scheduled.timeout, timeout))
    }

    /// Run the runnables queued on `context`, including those queued meanwhile: the port
    /// of `TestContext::execute`.
    pub fn run_runnables(&self, context: ContextType) {
        loop {
            let next = {
                let mut state = self.lock();
                let queue = &mut state.runnables[usize::from(context)];
                if queue.is_empty() { None } else { Some(queue.remove(0)) }
            };
            match next {
                Some(runnable) => self.run(context, runnable),
                None => return,
            }
        }
    }

    /// Run the timeouts of `context` that are due; a cyclic one is rescheduled first: the
    /// port of `TestContext::expire`.
    pub fn run_timeouts(&self, context: ContextType) {
        loop {
            let due = {
                let mut state = self.lock();
                let now = state.now;
                let index = state.timeouts.iter().position(|scheduled| {
                    scheduled.context == context && scheduled.elapse_at <= now
                });
                index.map(|index| state.timeouts.remove(index))
            };
            let Some(due) = due else { return };
            if due.period > 0 {
                self.insert(due.context, due.runnable, due.timeout, due.period, true);
            }
            self.run(context, due.runnable);
        }
    }

    /// Run timeouts and runnables of `context` until nothing is left for it: the port of
    /// `TestContext::expireAndExecute`.
    pub fn run_all_due(&self, context: ContextType) {
        loop {
            {
                let state = self.lock();
                let now = state.now;
                let has_due =
                    state.timeouts.iter().any(|s| s.context == context && s.elapse_at <= now);
                if state.runnables[usize::from(context)].is_empty() && !has_due {
                    return;
                }
            }
            self.run_timeouts(context);
            self.run_runnables(context);
        }
    }

    /// Run every context like a fixed-priority scheduler would, until all are idle: after
    /// each runnable, the highest-priority context with work runs next.
    pub fn run_until_idle(&self) {
        loop {
            let next = {
                let mut state = self.lock();
                let now = state.now;
                let mut next = None;
                for context in 0..N {
                    if let Some(index) = state
                        .timeouts
                        .iter()
                        .position(|s| usize::from(s.context) == context && s.elapse_at <= now)
                    {
                        let due = state.timeouts.remove(index);
                        next = Some((context as ContextType, due.runnable, Some(due)));
                        break;
                    }
                    if !state.runnables[context].is_empty() {
                        let runnable = state.runnables[context].remove(0);
                        next = Some((context as ContextType, runnable, None));
                        break;
                    }
                }
                next
            };
            let Some((context, runnable, due)) = next else { return };
            if let Some(due) = due
                && due.period > 0
            {
                self.insert(due.context, due.runnable, due.timeout, due.period, true);
            }
            self.run(context, runnable);
        }
    }

    fn run(&self, context: ContextType, runnable: &'static dyn Runnable) {
        let previous = {
            let mut state = self.lock();
            core::mem::replace(&mut state.current, context)
        };
        runnable.execute();
        self.lock().current = previous;
    }

    /// Enqueue at the end or right before the first entry with a strictly greater time.
    fn insert(
        &self,
        context: ContextType,
        runnable: &'static dyn Runnable,
        timeout: &'static Timeout,
        delay_us: u64,
        cyclic: bool,
    ) {
        let mut state = self.lock();
        if state.timeouts.iter().any(|scheduled| same_timeout(scheduled.timeout, timeout)) {
            return;
        }
        let elapse_at = state.now + delay_us;
        let position = state
            .timeouts
            .iter()
            .position(|scheduled| scheduled.elapse_at > elapse_at)
            .unwrap_or(state.timeouts.len());
        state.timeouts.insert(
            position,
            Scheduled {
                timeout,
                runnable,
                context,
                elapse_at,
                period: if cyclic { delay_us } else { 0 },
            },
        );
    }
}

impl<const N: usize> Async for MockAsync<N> {
    fn execute(&self, context: ContextType, runnable: &'static dyn Runnable) {
        let mut state = self.lock();
        let queue = &mut state.runnables[usize::from(context)];
        if !queue.iter().any(|queued| same_runnable(*queued, runnable)) {
            queue.push(runnable);
        }
    }

    fn schedule(
        &self,
        context: ContextType,
        runnable: &'static dyn Runnable,
        timeout: &'static Timeout,
        delay: u32,
        unit: TimeUnit,
    ) {
        self.insert(
            context,
            runnable,
            timeout,
            u64::from(delay) * u64::from(unit.microseconds()),
            false,
        );
    }

    fn schedule_at_fixed_rate(
        &self,
        context: ContextType,
        runnable: &'static dyn Runnable,
        timeout: &'static Timeout,
        period: u32,
        unit: TimeUnit,
    ) {
        self.insert(
            context,
            runnable,
            timeout,
            u64::from(period) * u64::from(unit.microseconds()),
            true,
        );
    }

    fn cancel(&self, timeout: &'static Timeout) {
        self.lock().timeouts.retain(|scheduled| !same_timeout(scheduled.timeout, timeout));
    }

    fn current_context(&self) -> ContextType {
        self.lock().current
    }

    fn task_name(&self, context: ContextType) -> &'static [u8] {
        self.names.get(usize::from(context)).copied().unwrap_or(b"<undefined>")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::QueueNode;
    use core::cell::RefCell;
    use std::boxed::Box;

    std::thread_local! {
        static LOG: RefCell<Vec<(u8, u64)>> = const { RefCell::new(Vec::new()) };
    }

    struct Tagged {
        tag: u8,
        mock: &'static MockAsync<2>,
        node: QueueNode<dyn Runnable>,
    }

    impl Runnable for Tagged {
        fn execute(&self) {
            LOG.with(|log| log.borrow_mut().push((self.tag, self.mock.now())));
        }

        fn node(&self) -> &QueueNode<dyn Runnable> {
            &self.node
        }
    }

    fn tagged(tag: u8, mock: &'static MockAsync<2>) -> &'static Tagged {
        Box::leak(Box::new(Tagged { tag, mock, node: QueueNode::new() }))
    }

    fn take_log() -> Vec<(u8, u64)> {
        LOG.with(|log| core::mem::take(&mut *log.borrow_mut()))
    }

    // Ported from async/test/src/async/TestContextTest.cpp.
    #[test]
    fn executes_queued_runnables_once() {
        let mock: &'static MockAsync<2> = Box::leak(Box::new(MockAsync::new([b"a", b"b"])));
        let r1 = tagged(1, mock);
        let r2 = tagged(2, mock);
        mock.execute(0, r1);
        mock.execute(0, r1);
        mock.execute(1, r2);
        assert!(mock.has_runnable(0));
        mock.run_runnables(0);
        assert_eq!(take_log(), [(1, 0)]);
        assert!(mock.has_runnable(1));
        mock.run_until_idle();
        assert_eq!(take_log(), [(2, 0)]);
    }

    #[test]
    fn expires_scheduled_and_cyclic_timeouts() {
        let mock: &'static MockAsync<2> = Box::leak(Box::new(MockAsync::new([b"a", b"b"])));
        let once = tagged(1, mock);
        let cyclic = tagged(2, mock);
        let t1: &'static Timeout = Box::leak(Box::new(Timeout::new()));
        let t2: &'static Timeout = Box::leak(Box::new(Timeout::new()));
        mock.schedule(0, once, t1, 5, TimeUnit::Milliseconds);
        mock.schedule_at_fixed_rate(0, cyclic, t2, 2, TimeUnit::Milliseconds);
        // Scheduling an already scheduled timeout changes nothing.
        mock.schedule(0, once, t1, 1, TimeUnit::Milliseconds);
        assert!(mock.is_scheduled(t1));
        mock.run_timeouts(0);
        assert!(take_log().is_empty());
        mock.set_now(2_000);
        mock.run_all_due(0);
        assert_eq!(take_log(), [(2, 2_000)]);
        mock.elapse(3_000);
        mock.run_timeouts(0);
        assert_eq!(take_log(), [(2, 5_000), (1, 5_000)]);
        assert!(!mock.is_scheduled(t1));
        mock.cancel(t2);
        assert!(!mock.is_scheduled(t2));
        mock.set_now(10_000);
        mock.run_timeouts(0);
        assert!(take_log().is_empty());
    }

    #[test]
    fn run_until_idle_prefers_the_highest_priority_context() {
        let mock: &'static MockAsync<2> = Box::leak(Box::new(MockAsync::new([b"high", b"low"])));
        struct Chain {
            mock: &'static MockAsync<2>,
            high: &'static Tagged,
            node: QueueNode<dyn Runnable>,
        }
        impl Runnable for Chain {
            fn execute(&self) {
                LOG.with(|log| log.borrow_mut().push((9, self.mock.current_context().into())));
                self.mock.execute(0, self.high);
            }
            fn node(&self) -> &QueueNode<dyn Runnable> {
                &self.node
            }
        }
        let high = tagged(1, mock);
        let low2 = tagged(2, mock);
        let chain: &'static Chain =
            Box::leak(Box::new(Chain { mock, high, node: QueueNode::new() }));
        mock.execute(1, chain);
        mock.execute(1, low2);
        mock.run_until_idle();
        // The low-priority chain queues a high-priority runnable, which runs before the
        // next low-priority one.
        assert_eq!(take_log(), [(9, 1), (1, 0), (2, 0)]);
        assert_eq!(mock.task_name(1), b"low");
        assert_eq!(mock.task_name(5), b"<undefined>");
    }
}
