// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Port of `libs/bsw/runtime`: who ran for how long.
//!
//! - [`RuntimeStatistics`] and [`FunctionRuntimeStatistics`]: total, count, minimum, maximum
//!   (and jitter) of run times (`RuntimeStatistics.h`, `FunctionRuntimeStatistics.h`).
//! - [`SimpleRuntimeEntry`], [`NestedRuntimeEntry`] and [`RuntimeStack`]: the stack of what is
//!   running, which charges a context's run time net of what preempted it
//!   (`RuntimeStackEntry.h`, `SimpleRuntimeEntry.h`, `NestedRuntimeEntry.h`,
//!   `RuntimeStack.h`).
//! - [`StatisticsContainer`] and [`StatisticsIterator`]: the entries of the tasks or
//!   interrupt groups, by name (`StatisticsContainer.h`, `StatisticsIterator.h`).
//! - [`RuntimeMonitor`]: fed by the context-switch and interrupt hooks
//!   (`RuntimeMonitor.h`).
//! - [`StatisticsWriter`]: the console table (`StatisticsWriter.h`).
//!
//! Not ported: `declare::RuntimeMonitor` and `declare::StatisticsContainer`, which bundle
//! the entry arrays with the object that refers to them (a Rust application declares the
//! arrays as their own `static`s); `SharedStatisticsContainer`, `FunctionExecutionMonitor`
//! and `Tracer`, which the demo does not use.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

mod container;
mod monitor;
mod stack;
mod statistics;
mod writer;

pub use container::{GetName, HasStatistics, StatisticsContainer, StatisticsIterator};
pub use monitor::{Clock, ContextEntry, FunctionEntry, RuntimeMonitor};
pub use stack::{NestedRuntimeEntry, RuntimeStack, SimpleRuntimeEntry, StackEntry};
pub use statistics::{FunctionRuntimeStatistics, RuntimeStatistics, Statistics};
pub use writer::{Mode, StatisticsWriter, TickConversion};
