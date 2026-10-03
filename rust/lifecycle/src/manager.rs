// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The lifecycle manager, ported from `LifecycleManager.h`/`.cpp` and
//! `ILifecycleListener.h`.

use core::cell::Cell;

use openbsw_async::{CONTEXT_INVALID, ContextType, Lock, QueueNode, Runnable};
use openbsw_util::{log_debug, log_info};

use crate::LIFECYCLE;
use crate::component::{LifecycleComponent, LifecycleComponentCallback, Transition};

/// Hears about every level reached: the port of `ILifecycleListener`.
pub trait LifecycleListener: Sync {
    /// `level` was reached by `transition` (run or shutdown).
    fn lifecycle_level_reached(&self, level: u8, transition: Transition);

    /// The node that links this listener into the manager's list.
    fn node(&self) -> &ListenerNode;
}

/// The link of a listener: the port of its `etl::forward_link`.
pub struct ListenerNode {
    next: Cell<Option<&'static dyn LifecycleListener>>,
}

// SAFETY: the list is changed only inside the manager's lock, and iterated with the lock
// taken around each step, as the C++ manager does.
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

/// What the manager records about a component: the port of
/// `LifecycleManager::ComponentInfo`.
pub struct ComponentInfo {
    name: Cell<&'static [u8]>,
    component: Cell<Option<&'static dyn LifecycleComponent>>,
    transition_times: [Cell<u32>; Transition::COUNT],
    is_transition_pending: Cell<bool>,
    last_transition: Cell<Transition>,
}

impl ComponentInfo {
    const fn new() -> Self {
        Self {
            name: Cell::new(b""),
            component: Cell::new(None),
            transition_times: [Cell::new(0), Cell::new(0), Cell::new(0)],
            is_transition_pending: Cell::new(false),
            last_transition: Cell::new(Transition::Init),
        }
    }

    /// The name used in logs.
    pub fn name(&self) -> &'static [u8] {
        self.name.get()
    }

    /// The component.
    pub fn component(&self) -> Option<&'static dyn LifecycleComponent> {
        self.component.get()
    }

    /// How long the last `transition` took, in the manager's time units.
    pub fn transition_time(&self, transition: Transition) -> u32 {
        self.transition_times[transition as usize].get()
    }

    /// Whether a transition was started and not yet reported done.
    pub fn is_transition_pending(&self) -> bool {
        self.is_transition_pending.get()
    }

    /// The transition started most recently.
    pub fn last_transition(&self) -> Transition {
        self.last_transition.get()
    }
}

/// One component's transition in flight, executed on the component's context: the port
/// of `ComponentTransitionExecutor`.
struct TransitionExecutor {
    component: Cell<Option<&'static dyn LifecycleComponent>>,
    component_index: Cell<u8>,
    transition: Cell<Transition>,
    is_pending: Cell<bool>,
    node: QueueNode<dyn Runnable>,
}

impl TransitionExecutor {
    const fn new() -> Self {
        Self {
            component: Cell::new(None),
            component_index: Cell::new(0),
            transition: Cell::new(Transition::Init),
            is_pending: Cell::new(false),
            node: QueueNode::new(),
        }
    }
}

// SAFETY: an executor is written by the manager on its own context before it is queued,
// and its pending flag cleared by `transition_done` from the component's context, the
// same unsynchronized handshake the C++ manager relies on.
unsafe impl Sync for TransitionExecutor {}

impl Runnable for TransitionExecutor {
    fn execute(&self) {
        if let Some(component) = self.component.get() {
            component.start_transition(self.transition.get());
        }
    }

    fn node(&self) -> &QueueNode<dyn Runnable> {
        &self.node
    }
}

/// Drives the components' transitions level by level: the port of
/// `declare::LifecycleManager<MAX_NUM_COMPONENTS, MAX_NUM_LEVELS, MAX_NUM_COMPONENTS_PER_LEVEL>`.
///
/// `L` is the critical section around the listener list. The manager is a runnable on its
/// transition context, so it must be a `'static` object (a `static` item).
pub struct LifecycleManager<
    const COMPONENTS: usize,
    const LEVELS: usize,
    const PER_LEVEL: usize,
    L: Lock + 'static,
> {
    component_infos: [ComponentInfo; COMPONENTS],
    executors: [TransitionExecutor; PER_LEVEL],
    executor_count: Cell<usize>,
    level_indices: [Cell<u8>; LEVELS],
    level_index_end: Cell<u8>,
    listeners: Cell<Option<&'static dyn LifecycleListener>>,
    get_timestamp: &'static (dyn Fn() -> u32 + Sync),
    transition_start_timestamp: Cell<u32>,
    transition_context: ContextType,
    is_transition_pending: Cell<bool>,
    transition: Cell<Transition>,
    transition_level: Cell<u8>,
    component_count: Cell<u8>,
    level_count: Cell<u8>,
    init_level_count: Cell<u8>,
    current_level: Cell<u8>,
    next_level: Cell<u8>,
    node: QueueNode<dyn Runnable>,
    _lock: core::marker::PhantomData<fn() -> L>,
}

// SAFETY: the manager's state is changed on its transition context, by `add_component`
// and `transition_to_level` during startup, and by `transition_done`, which the C++ code
// also calls from the component contexts without a lock; the listener list is guarded by
// `L`.
unsafe impl<const C: usize, const LV: usize, const P: usize, L: Lock + 'static> Sync
    for LifecycleManager<C, LV, P, L>
{
}

fn same_listener(a: &dyn LifecycleListener, b: &dyn LifecycleListener) -> bool {
    core::ptr::addr_eq(a, b)
}

fn same_component(a: &dyn LifecycleComponent, b: &dyn LifecycleComponent) -> bool {
    core::ptr::addr_eq(a, b)
}

impl<const COMPONENTS: usize, const LEVELS: usize, const PER_LEVEL: usize, L: Lock + 'static>
    LifecycleManager<COMPONENTS, LEVELS, PER_LEVEL, L>
{
    /// A manager whose own transitions run on `transition_context`, timing them with
    /// `get_timestamp`.
    pub const fn new(
        transition_context: ContextType,
        get_timestamp: &'static (dyn Fn() -> u32 + Sync),
    ) -> Self {
        Self {
            component_infos: [const { ComponentInfo::new() }; COMPONENTS],
            executors: [const { TransitionExecutor::new() }; PER_LEVEL],
            executor_count: Cell::new(0),
            level_indices: [const { Cell::new(0) }; LEVELS],
            level_index_end: Cell::new(0),
            listeners: Cell::new(None),
            get_timestamp,
            transition_start_timestamp: Cell::new(0),
            transition_context,
            is_transition_pending: Cell::new(false),
            transition: Cell::new(Transition::Init),
            transition_level: Cell::new(0),
            component_count: Cell::new(0),
            level_count: Cell::new(0),
            init_level_count: Cell::new(0),
            current_level: Cell::new(0),
            next_level: Cell::new(0),
            node: QueueNode::new(),
            _lock: core::marker::PhantomData,
        }
    }

    /// The number of level slots, `MAX_NUM_LEVELS + 1` in C++.
    const LEVEL_SLOTS: usize = LEVELS + 1;

    fn level_index(&self, level: usize) -> u8 {
        if level < LEVELS { self.level_indices[level].get() } else { self.level_index_end.get() }
    }

    fn set_level_index(&self, level: usize, index: u8) {
        if level < LEVELS {
            self.level_indices[level].set(index);
        } else {
            self.level_index_end.set(index);
        }
    }

    /// Add `component` at `level`, to be named `name` in logs. Components are added in
    /// ascending level order, during startup; the manager registers itself as the
    /// component's callback.
    pub fn add_component(
        &'static self,
        name: &'static [u8],
        component: &'static dyn LifecycleComponent,
        level: u8,
    ) {
        assert!(usize::from(self.component_count.get()) < COMPONENTS, "too many components");
        assert!(level > 0, "level must be greater than 0");
        assert!(usize::from(level) < Self::LEVEL_SLOTS, "level out of range");
        assert!(level >= self.level_count.get(), "components must be added by ascending levels");
        while self.level_count.get() < level {
            let count = self.level_count.get() + 1;
            self.level_count.set(count);
            self.set_level_index(usize::from(count), self.level_index(usize::from(count) - 1));
        }
        assert!(self.level_component_count(level) < PER_LEVEL, "too many components in a level");
        let info = &self.component_infos[usize::from(self.component_count.get())];
        info.name.set(name);
        info.component.set(Some(component));
        let count = usize::from(self.level_count.get());
        self.set_level_index(count, self.level_index(count) + 1);
        self.component_count.set(self.component_count.get() + 1);
        component.init_callback(self);
    }

    /// The number of registered components.
    pub fn component_count(&self) -> usize {
        usize::from(self.component_count.get())
    }

    /// The info of the component at internal index `idx`.
    pub fn component_info(&self, idx: usize) -> &ComponentInfo {
        &self.component_infos[idx]
    }

    /// The highest registered level.
    pub fn level_count(&self) -> u8 {
        self.level_count.get()
    }

    /// Transition to `level`: initialize and run every level up to it, or shut down the
    /// levels above it in descending order. A component is initialized at most once.
    pub fn transition_to_level(&'static self, level: u8) {
        self.next_level.set(if usize::from(level) < Self::LEVEL_SLOTS {
            level
        } else {
            (Self::LEVEL_SLOTS - 1) as u8
        });
        openbsw_async::execute(self.transition_context, self);
    }

    /// Add a listener, notified of every level reached.
    pub fn add_lifecycle_listener(&self, listener: &'static dyn LifecycleListener) {
        let _lock = L::lock();
        listener.node().next.set(self.listeners.get());
        self.listeners.set(Some(listener));
    }

    /// Remove a listener.
    pub fn remove_lifecycle_listener(&self, listener: &dyn LifecycleListener) {
        let _lock = L::lock();
        let mut prev: Option<&'static dyn LifecycleListener> = None;
        let mut current = self.listeners.get();
        while let Some(candidate) = current {
            if same_listener(candidate, listener) {
                let next = candidate.node().next.take();
                match prev {
                    None => self.listeners.set(next),
                    Some(prev) => prev.node().next.set(next),
                }
                return;
            }
            prev = Some(candidate);
            current = candidate.node().next.get();
        }
    }

    fn level_component_count(&self, level: u8) -> usize {
        usize::from(self.level_index(usize::from(level)) - self.level_index(usize::from(level) - 1))
    }

    fn as_static(&self) -> &'static Self {
        // SAFETY: the manager is a `'static` object (its public entry points take
        // `&'static self`, and a transition only runs after one of them), so extending the
        // borrow names the same object.
        unsafe { &*(self as *const Self) }
    }

    /// Start the next level transition if the current one is done: the port of
    /// `LifecycleManager::execute`.
    fn run_transition(&'static self) {
        if !self.check_level_transition_done()
            || (self.current_level.get() == self.next_level.get()
                && self.init_level_count.get() >= self.next_level.get())
        {
            return;
        }
        if self.next_level.get() < self.current_level.get() {
            self.transition_level.set(self.current_level.get());
            self.current_level.set(self.current_level.get() - 1);
            self.transition.set(Transition::Shutdown);
        } else if self.init_level_count.get() < self.next_level.get()
            && self.init_level_count.get() == self.current_level.get()
        {
            self.init_level_count.set(self.init_level_count.get() + 1);
            self.transition_level.set(self.init_level_count.get());
            self.transition.set(Transition::Init);
        } else {
            self.current_level.set(self.current_level.get() + 1);
            self.transition_level.set(self.current_level.get());
            self.transition.set(Transition::Run);
        }
        self.is_transition_pending.set(true);
        self.transition_start_timestamp.set((self.get_timestamp)());
        let transition = self.transition.get();
        let level = self.transition_level.get();
        log_info!(LIFECYCLE, b"%s level %d", transition.as_str(), level);
        let first = self.level_index(usize::from(level) - 1);
        let end = self.level_index(usize::from(level));
        for component_index in first..end {
            let info = &self.component_infos[usize::from(component_index)];
            let Some(component) = info.component.get() else { continue };
            let slot = self.executor_count.get();
            let executor = &self.executors[slot];
            self.executor_count.set(slot + 1);
            executor.component.set(Some(component));
            executor.component_index.set(component_index);
            executor.transition.set(transition);
            executor.is_pending.set(true);
            info.is_transition_pending.set(true);
            info.last_transition.set(transition);
            let mut context = component.transition_context(transition);
            if context == CONTEXT_INVALID {
                context = self.transition_context;
            }
            log_info!(LIFECYCLE, b"%s %s", transition.as_str(), info.name.get());
            openbsw_async::execute(context, executor);
        }
        if self.executor_count.get() == 0 {
            openbsw_async::execute(self.transition_context, self);
        }
    }

    fn check_level_transition_done(&self) -> bool {
        if !self.is_transition_pending.get() {
            return true;
        }
        if self.executors[..self.executor_count.get()]
            .iter()
            .any(|executor| executor.is_pending.get())
        {
            return false;
        }
        self.is_transition_pending.set(false);
        log_debug!(
            LIFECYCLE,
            b"%s level %d done",
            self.transition.get().as_str(),
            self.transition_level.get()
        );
        self.executor_count.set(0);
        if self.transition.get() != Transition::Init {
            let transition = self.transition.get();
            self.transition.set(Transition::Init);
            let mut current = {
                let _lock = L::lock();
                self.listeners.get()
            };
            while let Some(listener) = current {
                {
                    let _lock = L::lock();
                    current = listener.node().next.get();
                }
                listener.lifecycle_level_reached(self.current_level.get(), transition);
            }
        }
        true
    }
}

impl<const C: usize, const LV: usize, const P: usize, L: Lock + 'static> LifecycleComponentCallback
    for LifecycleManager<C, LV, P, L>
{
    fn transition_done(&self, component: &dyn LifecycleComponent) {
        for executor in &self.executors[..self.executor_count.get()] {
            if executor
                .component
                .get()
                .is_some_and(|candidate| same_component(candidate, component))
            {
                executor.is_pending.set(false);
                let info = &self.component_infos[usize::from(executor.component_index.get())];
                info.is_transition_pending.set(false);
                info.transition_times[executor.transition.get() as usize].set(
                    (self.get_timestamp)().wrapping_sub(self.transition_start_timestamp.get()),
                );
                log_debug!(
                    LIFECYCLE,
                    b"%s %s done",
                    executor.transition.get().as_str(),
                    info.name.get()
                );
                openbsw_async::execute(self.transition_context, self.as_static());
                break;
            }
        }
    }
}

impl<const C: usize, const LV: usize, const P: usize, L: Lock + 'static> Runnable
    for LifecycleManager<C, LV, P, L>
{
    fn execute(&self) {
        self.as_static().run_transition();
    }

    fn node(&self) -> &QueueNode<dyn Runnable> {
        &self.node
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use crate::component::ComponentBase;
    use openbsw_async::NoLock;
    use openbsw_async::mock::MockAsync;
    use openbsw_util::format::{Arg, PrintfFormatter};
    use openbsw_util::logger::{ComponentInfo as LoggerComponentInfo, PlainComponentInfo};
    use openbsw_util::logger::{ComponentMapping, Level, LevelInfo, Logger, LoggerOutput};
    use openbsw_util::stream::StringBufferOutputStream;
    use std::boxed::Box;
    use std::cell::RefCell;
    use std::string::String;
    use std::sync::Mutex;
    use std::vec::Vec;

    /// The tests share the async binding and the logger, so they run one at a time.
    static SERIAL: Mutex<()> = Mutex::new(());

    type Manager<const C: usize, const LV: usize, const P: usize> =
        LifecycleManager<C, LV, P, NoLock>;

    struct Clock(Cell<u32>);
    // SAFETY: single-threaded test object.
    unsafe impl Sync for Clock {}

    /// A component that records its transitions and finishes them on demand or at once.
    struct TestComponent {
        base: ComponentBase,
        calls: RefCell<Vec<Transition>>,
        auto_done: bool,
    }
    // SAFETY: single-threaded test object.
    unsafe impl Sync for TestComponent {}

    impl LifecycleComponent for TestComponent {
        fn base(&self) -> &ComponentBase {
            &self.base
        }
        fn init(&'static self) {
            self.calls.borrow_mut().push(Transition::Init);
            if self.auto_done {
                self.transition_done();
            }
        }
        fn run(&'static self) {
            self.calls.borrow_mut().push(Transition::Run);
            if self.auto_done {
                self.transition_done();
            }
        }
        fn shutdown(&'static self) {
            self.calls.borrow_mut().push(Transition::Shutdown);
            if self.auto_done {
                self.transition_done();
            }
        }
    }

    fn component(context: ContextType, auto_done: bool) -> &'static TestComponent {
        Box::leak(Box::new(TestComponent {
            base: ComponentBase::with_context(context),
            calls: RefCell::new(Vec::new()),
            auto_done,
        }))
    }

    struct Listener {
        reached: RefCell<Vec<(u8, Transition)>>,
        node: ListenerNode,
    }
    // SAFETY: single-threaded test object.
    unsafe impl Sync for Listener {}

    impl LifecycleListener for Listener {
        fn lifecycle_level_reached(&self, level: u8, transition: Transition) {
            self.reached.borrow_mut().push((level, transition));
        }
        fn node(&self) -> &ListenerNode {
            &self.node
        }
    }

    fn take<T: Clone>(cell: &RefCell<Vec<T>>) -> Vec<T> {
        core::mem::take(&mut *cell.borrow_mut())
    }

    // Ported from lifecycle/test/src/lifecycle/LifecycleManagerTest.cpp
    // (testCompleteLifecycle). The gmock timestamps become a clock the test advances; the
    // TestContext becomes MockAsync.
    #[test]
    fn complete_lifecycle() {
        let _guard = SERIAL.lock().unwrap();
        Logger::shutdown();
        let mock: &'static MockAsync<8> =
            Box::leak(Box::new(MockAsync::new([b"0", b"1", b"2", b"3", b"4", b"5", b"6", b"7"])));
        openbsw_async::set_binding(mock);
        let clock: &'static Clock = Box::leak(Box::new(Clock(Cell::new(0))));
        let now = Box::leak(Box::new(move || clock.0.get())) as &'static (dyn Fn() -> u32 + Sync);
        let cut: &'static Manager<4, 3, 2> = Box::leak(Box::new(Manager::new(1, now)));
        let listener: &'static Listener = Box::leak(Box::new(Listener {
            reached: RefCell::new(Vec::new()),
            node: ListenerNode::new(),
        }));
        cut.add_lifecycle_listener(listener);
        assert_eq!(cut.component_count(), 0);
        assert_eq!(cut.level_count(), 0);
        let c1 = component(CONTEXT_INVALID, false);
        let c2 = component(CONTEXT_INVALID, false);
        let c3 = component(CONTEXT_INVALID, false);
        cut.add_component(b"comp1", c1, 1);
        assert_eq!(cut.component_count(), 1);
        assert_eq!(cut.level_count(), 1);
        let info1 = cut.component_info(0);
        assert_eq!(info1.transition_time(Transition::Init), 0);
        assert_eq!(info1.last_transition(), Transition::Init);
        assert!(!info1.is_transition_pending());
        cut.add_component(b"comp2", c2, 1);
        assert_eq!(cut.level_count(), 1);
        let info2 = cut.component_info(1);
        cut.add_component(b"comp3", c3, 3);
        assert_eq!(cut.component_count(), 3);
        assert_eq!(cut.level_count(), 3);
        let info3 = cut.component_info(2);
        assert_eq!(info3.name(), b"comp3");
        // Nothing happens if transition_done is called accidentally.
        c3.transition_done();
        // Transition to level 4 (clamped to 3): level 1 is initialized.
        cut.transition_to_level(4);
        clock.0.set(100);
        mock.run_runnables(1);
        assert_eq!(take(&c1.calls), [Transition::Init]);
        assert_eq!(take(&c2.calls), [Transition::Init]);
        assert!(info1.is_transition_pending());
        assert!(info2.is_transition_pending());
        // Level 1 runs once both components report done.
        clock.0.set(200);
        c2.transition_done();
        assert!(!info2.is_transition_pending());
        assert_eq!(info2.transition_time(Transition::Init), 100);
        clock.0.set(300);
        c1.transition_done();
        assert_eq!(info1.transition_time(Transition::Init), 200);
        clock.0.set(400);
        mock.run_runnables(1);
        assert_eq!(take(&c1.calls), [Transition::Run]);
        assert_eq!(take(&c2.calls), [Transition::Run]);
        assert_eq!(info1.last_transition(), Transition::Run);
        clock.0.set(450);
        c2.transition_done();
        assert_eq!(info2.transition_time(Transition::Run), 50);
        mock.run_runnables(1);
        assert!(take(&c3.calls).is_empty());
        clock.0.set(500);
        c1.transition_done();
        assert_eq!(info1.transition_time(Transition::Run), 100);
        // Level 1 reached, level 2 (empty) initialized and run, level 3 initialized.
        clock.0.set(530);
        mock.run_runnables(1);
        assert_eq!(take(&listener.reached), [(1, Transition::Run), (2, Transition::Run)]);
        assert_eq!(take(&c3.calls), [Transition::Init]);
        clock.0.set(600);
        c3.transition_done();
        clock.0.set(620);
        mock.run_runnables(1);
        assert_eq!(take(&c3.calls), [Transition::Run]);
        // Transition to level 1 even before level 3 has been reached.
        cut.transition_to_level(1);
        clock.0.set(680);
        c3.transition_done();
        clock.0.set(700);
        mock.run_runnables(1);
        assert_eq!(take(&listener.reached), [(3, Transition::Run)]);
        assert_eq!(take(&c3.calls), [Transition::Shutdown]);
        assert!(info3.is_transition_pending());
        assert_eq!(info3.last_transition(), Transition::Shutdown);
        clock.0.set(750);
        c3.transition_done();
        assert_eq!(info3.transition_time(Transition::Shutdown), 50);
        mock.run_runnables(1);
        assert_eq!(take(&listener.reached), [(2, Transition::Shutdown), (1, Transition::Shutdown)]);
        // Transition to level 0: comp2 shuts down on another context.
        c2.base().set_transition_context(7);
        cut.transition_to_level(0);
        clock.0.set(800);
        mock.run_runnables(1);
        assert_eq!(take(&c1.calls), [Transition::Shutdown]);
        assert!(take(&c2.calls).is_empty());
        mock.run_runnables(7);
        assert_eq!(take(&c2.calls), [Transition::Shutdown]);
        clock.0.set(820);
        c1.transition_done();
        clock.0.set(830);
        c2.transition_done();
        mock.run_runnables(1);
        assert_eq!(take(&listener.reached), [(0, Transition::Shutdown)]);
        // No initialization on the next transition to level 1.
        c2.base().set_transition_context(CONTEXT_INVALID);
        cut.transition_to_level(1);
        mock.run_runnables(1);
        assert_eq!(take(&c1.calls), [Transition::Run]);
        assert_eq!(take(&c2.calls), [Transition::Run]);
        cut.remove_lifecycle_listener(listener);
        c1.transition_done();
        c2.transition_done();
        mock.run_runnables(1);
        assert!(take(&listener.reached).is_empty());
    }

    fn manager<const C: usize, const LV: usize, const P: usize>() -> &'static Manager<C, LV, P> {
        Box::leak(Box::new(Manager::new(1, &|| 0)))
    }

    #[test]
    #[should_panic(expected = "level must be greater than 0")]
    fn add_component_asserts_level_greater_0() {
        manager::<4, 4, 4>().add_component(b"comp1", component(0, true), 0);
    }

    #[test]
    #[should_panic(expected = "too many components")]
    fn add_component_asserts_not_too_many_components() {
        let cut = manager::<3, 4, 4>();
        cut.add_component(b"comp1", component(0, true), 1);
        cut.add_component(b"comp2", component(0, true), 2);
        cut.add_component(b"comp3", component(0, true), 3);
        cut.add_component(b"comp4", component(0, true), 4);
    }

    #[test]
    #[should_panic(expected = "too many components in a level")]
    fn add_component_asserts_not_too_many_components_per_level() {
        let cut = manager::<3, 2, 2>();
        cut.add_component(b"comp1", component(0, true), 1);
        cut.add_component(b"comp2", component(0, true), 1);
        cut.add_component(b"comp3", component(0, true), 1);
    }

    #[test]
    #[should_panic(expected = "level out of range")]
    fn add_component_asserts_not_too_many_levels() {
        let cut = manager::<3, 4, 2>();
        cut.add_component(b"comp1", component(0, true), 1);
        cut.add_component(b"comp2", component(0, true), 4);
        cut.add_component(b"comp3", component(0, true), 5);
    }

    #[test]
    #[should_panic(expected = "ascending levels")]
    fn add_component_asserts_ascending_levels() {
        let cut = manager::<4, 4, 4>();
        cut.add_component(b"comp1", component(0, true), 1);
        cut.add_component(b"comp2", component(0, true), 1);
        cut.add_component(b"comp3", component(0, true), 2);
        cut.add_component(b"comp4", component(0, true), 1);
    }

    /// A logger output that formats every line like the demo's console, without the
    /// timestamp prefix.
    struct Transcript(RefCell<Vec<String>>);
    // SAFETY: single-threaded test object.
    unsafe impl Sync for Transcript {}

    static LIFECYCLE_INFO: PlainComponentInfo = PlainComponentInfo::new(
        b"LIFECYCLE",
        openbsw_util::format::StringAttributes::color(openbsw_util::format::Color::DarkGray),
    );

    impl ComponentMapping for Transcript {
        fn is_enabled(&self, _component_index: u8, _level: Level) -> bool {
            true
        }
        fn level(&self, _component_index: u8) -> Level {
            Level::Debug
        }
        fn level_info(&self, level: Level) -> LevelInfo {
            LevelInfo::new(Some(&LevelInfo::default_table()[level as usize]))
        }
        fn component_info(&self, component_index: u8) -> LoggerComponentInfo {
            LoggerComponentInfo::new(component_index, Some(&LIFECYCLE_INFO))
        }
    }

    impl LoggerOutput for Transcript {
        fn log_output(
            &self,
            component_info: &LoggerComponentInfo,
            level_info: &LevelInfo,
            format: &[u8],
            args: &[Arg<'_>],
        ) {
            let mut buffer = [0u8; 200];
            let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
            PrintfFormatter::new(&mut stream, true).format_args(format, args);
            let line = std::format!(
                "{}: {}: {}",
                String::from_utf8_lossy(component_info.name().string()),
                String::from_utf8_lossy(level_info.name().string()),
                String::from_utf8_lossy(stream.string())
            );
            self.0.borrow_mut().push(line);
        }
    }

    /// The demo's boot: seven components over eight levels (level 3 empty), each on its
    /// own context, driven like the fixed-priority scheduler would. The lines must match
    /// the C++ demo's console after its timestamp prefix.
    #[test]
    fn demo_boot_transcript() {
        let _guard = SERIAL.lock().unwrap();
        let mock: &'static MockAsync<5> = Box::leak(Box::new(MockAsync::new([
            b"sysadmin",
            b"can",
            b"demo",
            b"uds",
            b"background",
        ])));
        openbsw_async::set_binding(mock);
        let transcript: &'static Transcript =
            Box::leak(Box::new(Transcript(RefCell::new(Vec::new()))));
        LIFECYCLE.set_index(0);
        Logger::init(transcript, transcript);
        let cut: &'static Manager<16, 8, 16> = Box::leak(Box::new(Manager::new(0, &|| 0)));
        let components: [(&[u8], u8, ContextType); 7] = [
            (b"runtime", 1, 4),
            (b"can", 2, 1),
            (b"transport", 4, 3),
            (b"docan", 5, 1),
            (b"uds", 6, 3),
            (b"sysadmin", 7, 0),
            (b"demo", 8, 2),
        ];
        for (name, level, context) in components {
            cut.add_component(name, component(context, true), level);
        }
        cut.transition_to_level(8);
        mock.run_until_idle();
        Logger::shutdown();
        let lines = take(&transcript.0);
        let mut expected = Vec::new();
        let names_by_level: [&[&str]; 8] = [
            &["runtime"],
            &["can"],
            &[],
            &["transport"],
            &["docan"],
            &["uds"],
            &["sysadmin"],
            &["demo"],
        ];
        for (level, names) in names_by_level.iter().enumerate() {
            for transition in ["Initialize", "Run"] {
                expected.push(std::format!("LIFECYCLE: INFO: {transition} level {}", level + 1));
                for name in names.iter() {
                    expected.push(std::format!("LIFECYCLE: INFO: {transition} {name}"));
                    expected.push(std::format!("LIFECYCLE: DEBUG: {transition} {name} done"));
                }
                expected
                    .push(std::format!("LIFECYCLE: DEBUG: {transition} level {} done", level + 1));
            }
        }
        assert_eq!(lines, expected);
    }
}
