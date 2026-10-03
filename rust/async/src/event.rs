// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Event dispatching, ported from `async/EventDispatcher.h` and `async/EventPolicy.h`.

use core::cell::Cell;

use openbsw_timer::Lock;

use crate::types::EventMaskType;

/// Handles one event of a dispatcher.
pub trait EventHandler: Sync {
    /// The event was set.
    fn handle_event(&self);
}

/// What an [`EventPolicy`] talks to: a task context that stores handlers and signals
/// events to its task.
pub trait Dispatcher: Sync {
    /// Install the handler of `event`.
    fn set_event_handler(&self, event: usize, handler: &'static dyn EventHandler);

    /// Remove the handler of `event`.
    fn remove_event_handler(&self, event: usize);

    /// Signal the events in `mask` to the task.
    fn set_events(&self, mask: EventMaskType);
}

/// The handlers of `N` events, called in event order: the port of
/// `EventDispatcher<EventCount, Lock>`.
pub struct EventDispatcher<const N: usize, L: Lock> {
    handlers: [Cell<Option<&'static dyn EventHandler>>; N],
    _lock: core::marker::PhantomData<L>,
}

// SAFETY: the handler table is changed only inside `L`, and read by the task that owns the
// dispatcher; the platform's critical section serializes the sharing.
unsafe impl<const N: usize, L: Lock> Sync for EventDispatcher<N, L> {}

impl<const N: usize, L: Lock> EventDispatcher<N, L> {
    /// The number of events.
    pub const EVENT_COUNT: usize = N;

    /// A dispatcher with no handlers.
    pub const fn new() -> Self {
        Self { handlers: [const { Cell::new(None) }; N], _lock: core::marker::PhantomData }
    }

    /// Install the handler of `event`.
    pub fn set_event_handler(&self, event: usize, handler: &'static dyn EventHandler) {
        let _lock = L::lock();
        self.handlers[event].set(Some(handler));
    }

    /// Remove the handler of `event`.
    pub fn remove_event_handler(&self, event: usize) {
        let _lock = L::lock();
        self.handlers[event].set(None);
    }

    /// Call the handler of every event in `mask`, lowest event first.
    pub fn handle_events(&self, mask: EventMaskType) {
        for (event, handler) in self.handlers.iter().enumerate() {
            if (mask & (1 << event)) != 0
                && let Some(handler) = handler.get()
            {
                handler.handle_event();
            }
        }
    }
}

impl<const N: usize, L: Lock> Default for EventDispatcher<N, L> {
    fn default() -> Self {
        Self::new()
    }
}

/// One event of a dispatcher, as its handler sees it: the port of
/// `EventPolicy<EventDispatcher, Event>`.
pub struct EventPolicy<D: Dispatcher + ?Sized + 'static, const EVENT: usize> {
    dispatcher: &'static D,
}

impl<D: Dispatcher + ?Sized + 'static, const EVENT: usize> EventPolicy<D, EVENT> {
    /// The bit of the event.
    pub const EVENT_MASK: EventMaskType = 1 << EVENT;

    /// The policy for `EVENT` of `dispatcher`.
    pub const fn new(dispatcher: &'static D) -> Self {
        Self { dispatcher }
    }

    /// Install `handler` for the event.
    pub fn set_event_handler(&self, handler: &'static dyn EventHandler) {
        self.dispatcher.set_event_handler(EVENT, handler);
    }

    /// Remove the event's handler.
    pub fn remove_event_handler(&self) {
        self.dispatcher.remove_event_handler(EVENT);
    }

    /// Signal the event.
    pub fn set_event(&self) {
        self.dispatcher.set_events(Self::EVENT_MASK);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use core::cell::RefCell;
    use openbsw_timer::NoLock;
    use std::boxed::Box;
    use std::vec::Vec;

    std::thread_local! {
        pub(crate) static CALLS: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    }

    pub(crate) struct Tagged(pub(crate) u8);

    impl EventHandler for Tagged {
        fn handle_event(&self) {
            CALLS.with(|calls| calls.borrow_mut().push(self.0));
        }
    }

    pub(crate) fn tagged(tag: u8) -> &'static Tagged {
        Box::leak(Box::new(Tagged(tag)))
    }

    pub(crate) fn take_calls() -> Vec<u8> {
        CALLS.with(|calls| core::mem::take(&mut *calls.borrow_mut()))
    }

    // Ported from asyncImpl/test/src/async/EventDispatcherTest.cpp.
    #[test]
    fn event_dispatcher() {
        let cut: EventDispatcher<3, NoLock> = EventDispatcher::new();
        cut.set_event_handler(0, tagged(1));
        cut.set_event_handler(2, tagged(3));
        cut.handle_events(7);
        assert_eq!(take_calls(), [1, 3]);
        cut.set_event_handler(1, tagged(2));
        cut.handle_events(7);
        assert_eq!(take_calls(), [1, 2, 3]);
        cut.handle_events(4);
        assert_eq!(take_calls(), [3]);
        cut.remove_event_handler(2);
        cut.handle_events(7);
        assert_eq!(take_calls(), [1, 2]);
    }

    /// Records what the policy forwards, the port of the EventPolicyTest mock.
    pub(crate) struct RecordingDispatcher {
        pub(crate) set: RefCell<Vec<(usize, u8)>>,
        pub(crate) removed: RefCell<Vec<usize>>,
        pub(crate) events: RefCell<Vec<EventMaskType>>,
    }

    // SAFETY: single-threaded test object.
    unsafe impl Sync for RecordingDispatcher {}

    impl RecordingDispatcher {
        pub(crate) fn leak() -> &'static Self {
            Box::leak(Box::new(Self {
                set: RefCell::new(Vec::new()),
                removed: RefCell::new(Vec::new()),
                events: RefCell::new(Vec::new()),
            }))
        }
    }

    impl Dispatcher for RecordingDispatcher {
        fn set_event_handler(&self, event: usize, handler: &'static dyn EventHandler) {
            handler.handle_event();
            let tag = take_calls().pop().unwrap_or(0);
            self.set.borrow_mut().push((event, tag));
        }

        fn remove_event_handler(&self, event: usize) {
            self.removed.borrow_mut().push(event);
        }

        fn set_events(&self, mask: EventMaskType) {
            self.events.borrow_mut().push(mask);
        }
    }

    // Ported from asyncImpl/test/src/async/EventPolicyTest.cpp.
    #[test]
    fn event_policy() {
        let dispatcher = RecordingDispatcher::leak();
        let cut: EventPolicy<RecordingDispatcher, 1> = EventPolicy::new(dispatcher);
        cut.set_event_handler(tagged(9));
        assert_eq!(dispatcher.set.borrow().as_slice(), [(1, 9)]);
        cut.remove_event_handler();
        assert_eq!(dispatcher.removed.borrow().as_slice(), [1]);
        cut.set_event();
        assert_eq!(dispatcher.events.borrow().as_slice(), [1 << 1]);
    }
}
