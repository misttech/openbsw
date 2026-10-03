// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Port of Eclipse OpenBSW's `libs/bsw/lifecycle`.
//!
//! A [`LifecycleManager`] holds components registered at run levels and drives their
//! transitions: going up, every level is initialized and then run before the next one;
//! going down, levels are shut down in descending order. Each transition runs as a
//! runnable on the component's async context (or the manager's), and the component
//! reports [`transition_done`](LifecycleComponentCallback::transition_done) when it is
//! finished, which lets the manager continue. Listeners hear about every level reached.
//!
//! Every step is logged on the `LIFECYCLE` component with the C++ strings, which the
//! demo's console transcript is made of.
//!
//! Not ported: `LifecycleManagerForwarder` and `LifecycleManagerInitializer`, template
//! sugar the demo does not use.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

mod component;
mod manager;

pub use component::{ComponentBase, LifecycleComponent, LifecycleComponentCallback, Transition};
pub use manager::{ComponentInfo, LifecycleListener, LifecycleManager, ListenerNode};

use openbsw_util::logger::LoggerComponent;

/// The `LIFECYCLE` logger component.
pub static LIFECYCLE: LoggerComponent = LoggerComponent::new();
