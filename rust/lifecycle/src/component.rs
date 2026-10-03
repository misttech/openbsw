// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Lifecycle components, ported from `ILifecycleComponent.h`, `LifecycleComponent.h`,
//! `AsyncLifecycleComponent.h`, `SingleContextLifecycleComponent.h` and
//! `SimpleLifecycleComponent.h`.
//!
//! The C++ base classes differ only in how they answer `getTransitionContext`; the port
//! merges them into [`ComponentBase`], whose constructors cover the three cases.

use core::cell::Cell;

use openbsw_async::{CONTEXT_INVALID, ContextType};

/// A transition of a component or level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Transition {
    /// Initialization.
    Init = 0,
    /// Running.
    Run = 1,
    /// Shutdown.
    Shutdown = 2,
}

impl Transition {
    /// The number of transition kinds.
    pub const COUNT: usize = 3;

    /// The word the lifecycle logs use for the transition.
    pub fn as_str(self) -> &'static [u8] {
        match self {
            Transition::Run => b"Run",
            Transition::Shutdown => b"Shutdown",
            Transition::Init => b"Initialize",
        }
    }
}

/// Where a component reports that its transition is finished: the port of
/// `ILifecycleComponentCallback`.
pub trait LifecycleComponentCallback: Sync {
    /// `component` finished the transition the callback started.
    fn transition_done(&self, component: &dyn LifecycleComponent);
}

/// The state every component carries: the callback and the async context of each
/// transition.
pub struct ComponentBase {
    callback: Cell<Option<&'static dyn LifecycleComponentCallback>>,
    contexts: [Cell<ContextType>; Transition::COUNT],
}

// SAFETY: the callback is set once, when the component is added to its manager during
// startup; the contexts are set by the component's constructor or startup code and read
// by the manager afterwards. Nothing writes them while the tasks run.
unsafe impl Sync for ComponentBase {}

impl ComponentBase {
    /// Transitions run on the manager's context (`SimpleLifecycleComponent`, and the
    /// default of `AsyncLifecycleComponent`).
    pub const fn new() -> Self {
        Self::with_context(CONTEXT_INVALID)
    }

    /// All transitions run on `context` (`SingleContextLifecycleComponent`).
    pub const fn with_context(context: ContextType) -> Self {
        Self {
            callback: Cell::new(None),
            contexts: [Cell::new(context), Cell::new(context), Cell::new(context)],
        }
    }

    /// Set the context of all transitions.
    pub fn set_transition_context(&self, context: ContextType) {
        for cell in &self.contexts {
            cell.set(context);
        }
    }

    /// Set the context of one transition.
    pub fn set_transition_context_for(&self, transition: Transition, context: ContextType) {
        self.contexts[transition as usize].set(context);
    }

    /// The context of `transition`.
    pub fn transition_context(&self, transition: Transition) -> ContextType {
        self.contexts[transition as usize].get()
    }

    /// Store the callback to report finished transitions to.
    pub fn init_callback(&self, callback: &'static dyn LifecycleComponentCallback) {
        self.callback.set(Some(callback));
    }

    /// Report that `component` finished its transition.
    pub fn transition_done(&self, component: &dyn LifecycleComponent) {
        if let Some(callback) = self.callback.get() {
            callback.transition_done(component);
        }
    }
}

impl Default for ComponentBase {
    fn default() -> Self {
        Self::new()
    }
}

/// A component managed by a [`super::LifecycleManager`]: the port of `ILifecycleComponent`
/// together with the `LifecycleComponent` base.
///
/// A component implements `init`, `run` and `shutdown` and embeds a [`ComponentBase`];
/// it calls [`transition_done`](Self::transition_done) when a transition is finished,
/// typically at the end of the transition function.
///
/// The transitions take `&'static self`: a component is a `'static` object (the manager
/// holds it as one), and a transition typically hands the component itself to the async
/// binding as a runnable or a timeout owner, which needs the `'static` borrow.
pub trait LifecycleComponent: Sync {
    /// The embedded state.
    fn base(&self) -> &ComponentBase;

    /// Initialize.
    fn init(&'static self);

    /// Run.
    fn run(&'static self);

    /// Shut down.
    fn shutdown(&'static self);

    /// Register the callback on which to call `transition_done`. The manager does this
    /// when the component is added.
    fn init_callback(&self, callback: &'static dyn LifecycleComponentCallback) {
        self.base().init_callback(callback);
    }

    /// The context `transition` runs on; `CONTEXT_INVALID` means the manager's.
    fn transition_context(&self, transition: Transition) -> ContextType {
        self.base().transition_context(transition)
    }

    /// Perform `transition`. Called by the manager on the transition's context.
    fn start_transition(&'static self, transition: Transition) {
        match transition {
            Transition::Init => self.init(),
            Transition::Run => self.run(),
            Transition::Shutdown => self.shutdown(),
        }
    }

    /// Report that the current transition is finished.
    fn transition_done(&self)
    where
        Self: Sized,
    {
        self.base().transition_done(self);
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use std::boxed::Box;
    use std::cell::RefCell;
    use std::vec::Vec;

    struct Recorder {
        calls: RefCell<Vec<&'static str>>,
        done: RefCell<usize>,
    }

    // SAFETY: single-threaded test object.
    unsafe impl Sync for Recorder {}

    impl LifecycleComponentCallback for Recorder {
        fn transition_done(&self, _component: &dyn LifecycleComponent) {
            *self.done.borrow_mut() += 1;
        }
    }

    struct Component {
        base: ComponentBase,
        recorder: &'static Recorder,
    }

    impl LifecycleComponent for Component {
        fn base(&self) -> &ComponentBase {
            &self.base
        }

        fn init(&'static self) {
            self.recorder.calls.borrow_mut().push("init");
        }

        fn run(&'static self) {
            self.recorder.calls.borrow_mut().push("run");
        }

        fn shutdown(&'static self) {
            self.recorder.calls.borrow_mut().push("shutdown");
        }
    }

    fn recorder() -> &'static Recorder {
        Box::leak(Box::new(Recorder { calls: RefCell::new(Vec::new()), done: RefCell::new(0) }))
    }

    // Ported from lifecycle/test/src/lifecycle/LifecycleComponentTest.cpp.
    #[test]
    fn complete_component() {
        let recorder = recorder();
        let cut: &'static Component =
            Box::leak(Box::new(Component { base: ComponentBase::new(), recorder }));
        cut.init_callback(recorder);
        for (transition, name) in [
            (Transition::Init, "init"),
            (Transition::Run, "run"),
            (Transition::Shutdown, "shutdown"),
        ] {
            cut.start_transition(transition);
            assert_eq!(recorder.calls.borrow_mut().pop(), Some(name));
            let before = *recorder.done.borrow();
            cut.transition_done();
            assert_eq!(*recorder.done.borrow(), before + 1);
        }
    }

    #[test]
    fn transition_done_without_callback_does_nothing() {
        let cut: &'static Component =
            Box::leak(Box::new(Component { base: ComponentBase::new(), recorder: recorder() }));
        cut.transition_done();
    }

    // Ported from AsyncLifecycleComponentTest.cpp.
    #[test]
    fn async_component_contexts() {
        let cut = ComponentBase::new();
        for transition in [Transition::Init, Transition::Run, Transition::Shutdown] {
            assert_eq!(cut.transition_context(transition), CONTEXT_INVALID);
        }
        cut.set_transition_context_for(Transition::Init, 1);
        cut.set_transition_context_for(Transition::Run, 2);
        cut.set_transition_context_for(Transition::Shutdown, 3);
        assert_eq!(cut.transition_context(Transition::Init), 1);
        assert_eq!(cut.transition_context(Transition::Run), 2);
        assert_eq!(cut.transition_context(Transition::Shutdown), 3);
        let cut = ComponentBase::new();
        cut.set_transition_context(2);
        for transition in [Transition::Init, Transition::Run, Transition::Shutdown] {
            assert_eq!(cut.transition_context(transition), 2);
        }
    }

    // Ported from SingleContextLifecycleComponentTest.cpp and SimpleLifecycleComponentTest.cpp.
    #[test]
    fn single_context_and_simple_components() {
        let cut = ComponentBase::with_context(CONTEXT_INVALID);
        for transition in [Transition::Init, Transition::Run, Transition::Shutdown] {
            assert_eq!(cut.transition_context(transition), CONTEXT_INVALID);
        }
        let cut = ComponentBase::with_context(2);
        for transition in [Transition::Init, Transition::Run, Transition::Shutdown] {
            assert_eq!(cut.transition_context(transition), 2);
        }
        assert_eq!(Transition::Init.as_str(), b"Initialize");
        assert_eq!(Transition::Run.as_str(), b"Run");
        assert_eq!(Transition::Shutdown.as_str(), b"Shutdown");
    }
}
