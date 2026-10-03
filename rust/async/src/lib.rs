// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Port of Eclipse OpenBSW's `libs/bsw/async` and `libs/bsw/asyncImpl`, with the
//! platform-independent types of the Zephyr adaptation (`async/Types.h`).
//!
//! The async model: a fixed set of *contexts* (one task each, in priority order), on which
//! a [`Runnable`] is executed once ([`execute`]) or after a delay ([`schedule`],
//! [`schedule_at_fixed_rate`]). The platform binding, an [`Async`] implementation
//! installed with [`set_binding`], owns the tasks; this crate holds the building blocks it
//! is made of: the intrusive runnable [`queue`], the [`event`] dispatcher and the
//! [`executor`] that drains a context's queue.
//!
//! With the `mock` feature, [`mock::MockAsync`] runs the model on the host for tests.

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

pub mod binding;
pub mod call;
pub mod event;
pub mod executor;
pub mod lock;
#[cfg(feature = "mock")]
pub mod mock;
pub mod queue;
pub mod types;

pub use binding::{
    Async, cancel, current_context, execute, schedule, schedule_at_fixed_rate, set_binding,
};
pub use call::FunctionRunnable;
pub use event::{Dispatcher, EventDispatcher, EventHandler, EventPolicy};
pub use executor::RunnableExecutor;
pub use lock::{ModifiableLock, NoRawLock, RawLock, ScopedLock};
pub use openbsw_timer::{Lock, NoLock};
pub use queue::{HasQueueNode, Queue, QueueNode};
pub use types::{CONTEXT_INVALID, ContextType, EventMaskType, Runnable, TimeUnit, Timeout};
