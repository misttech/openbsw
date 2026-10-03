// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The runnable queue of a context, ported from `async/RunnableExecutor.h`.

use openbsw_timer::Lock;

use crate::event::{Dispatcher, EventHandler, EventPolicy};
use crate::queue::Queue;
use crate::types::Runnable;

/// Queues runnables and signals its event; when the event is handled, executes them all
/// in order: the port of `RunnableExecutor<Runnable, EventPolicy, Lock>`.
pub struct RunnableExecutor<D: Dispatcher + ?Sized + 'static, L: Lock, const EVENT: usize> {
    queue: Queue<dyn Runnable>,
    policy: EventPolicy<D, EVENT>,
    _lock: core::marker::PhantomData<fn() -> L>,
}

impl<D: Dispatcher + ?Sized + 'static, L: Lock, const EVENT: usize> RunnableExecutor<D, L, EVENT> {
    /// An executor signalling `EVENT` on `dispatcher`.
    pub const fn new(dispatcher: &'static D) -> Self {
        Self {
            queue: Queue::new(),
            policy: EventPolicy::new(dispatcher),
            _lock: core::marker::PhantomData,
        }
    }

    /// Install this executor as the handler of its event.
    pub fn init(&'static self) {
        self.policy.set_event_handler(self);
    }

    /// Remove the handler.
    pub fn shutdown(&self) {
        self.policy.remove_event_handler();
    }

    /// Queue `runnable` (once, if it is already queued) and signal the event.
    pub fn enqueue(&self, runnable: &'static dyn Runnable) {
        {
            let _lock = L::lock();
            if !runnable.node().is_enqueued() {
                self.queue.enqueue(runnable);
            }
        }
        self.policy.set_event();
    }
}

impl<D: Dispatcher + ?Sized + 'static, L: Lock, const EVENT: usize> EventHandler
    for RunnableExecutor<D, L, EVENT>
{
    fn handle_event(&self) {
        loop {
            let runnable = {
                let _lock = L::lock();
                self.queue.dequeue()
            };
            match runnable {
                Some(runnable) => runnable.execute(),
                None => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::tests::{RecordingDispatcher, take_calls};
    use crate::queue::QueueNode;
    use openbsw_timer::NoLock;
    use std::boxed::Box;

    struct TestRunnable {
        tag: u8,
        node: QueueNode<dyn Runnable>,
    }

    impl Runnable for TestRunnable {
        fn execute(&self) {
            crate::event::tests::CALLS.with(|calls| calls.borrow_mut().push(self.tag));
        }

        fn node(&self) -> &QueueNode<dyn Runnable> {
            &self.node
        }
    }

    fn runnable(tag: u8) -> &'static TestRunnable {
        Box::leak(Box::new(TestRunnable { tag, node: QueueNode::new() }))
    }

    // Ported from asyncImpl/test/src/async/RunnableExecutorTest.cpp.
    #[test]
    fn runnable_executor() {
        let dispatcher = RecordingDispatcher::leak();
        let cut: &'static RunnableExecutor<RecordingDispatcher, NoLock, 2> =
            Box::leak(Box::new(RunnableExecutor::new(dispatcher)));
        cut.init();
        assert_eq!(dispatcher.set.borrow().len(), 1);
        assert_eq!(dispatcher.set.borrow()[0].0, 2);
        take_calls();
        let (r1, r2, r3) = (runnable(1), runnable(2), runnable(3));
        cut.enqueue(r1);
        cut.enqueue(r2);
        cut.enqueue(r3);
        // Enqueuing an enqueued runnable signals again but does not queue it twice.
        cut.enqueue(r1);
        assert_eq!(dispatcher.events.borrow().as_slice(), [4, 4, 4, 4]);
        cut.handle_event();
        assert_eq!(take_calls(), [1, 2, 3]);
        cut.handle_event();
        assert!(take_calls().is_empty());
        cut.shutdown();
        assert_eq!(dispatcher.removed.borrow().as_slice(), [2]);
    }
}
