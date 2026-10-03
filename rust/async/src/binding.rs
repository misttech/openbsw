// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The platform binding, ported from `async/Async.h` and the adaptation's `Async.cpp`.
//!
//! C++ resolves `async::execute` to the adapter at link time through `AsyncBinding.h`.
//! Here the adapter installs itself with [`set_binding`] at startup; the free functions
//! forward to it, and do nothing until it is installed.

use openbsw_util::cell::RacyCell;

use crate::types::{CONTEXT_INVALID, ContextType, Runnable, TimeUnit, Timeout};

/// What the platform adapter provides: the port of `ZephyrAdapter`'s static interface.
pub trait Async: Sync {
    /// Execute `runnable` once on `context`.
    fn execute(&self, context: ContextType, runnable: &'static dyn Runnable);

    /// Execute `runnable` on `context` after `delay` units, through `timeout`.
    fn schedule(
        &self,
        context: ContextType,
        runnable: &'static dyn Runnable,
        timeout: &'static Timeout,
        delay: u32,
        unit: TimeUnit,
    );

    /// Execute `runnable` on `context` every `period` units, through `timeout`.
    fn schedule_at_fixed_rate(
        &self,
        context: ContextType,
        runnable: &'static dyn Runnable,
        timeout: &'static Timeout,
        period: u32,
        unit: TimeUnit,
    );

    /// Cancel `timeout`.
    fn cancel(&self, timeout: &'static Timeout);

    /// The context of the calling task, `CONTEXT_INVALID` in an interrupt.
    fn current_context(&self) -> ContextType;

    /// The name of the task of `context`.
    fn task_name(&self, context: ContextType) -> &'static [u8];
}

static BINDING: RacyCell<Option<&'static dyn Async>> = RacyCell::new(None);

/// Install the platform binding. Call during startup, before anything uses the binding.
pub fn set_binding(binding: &'static dyn Async) {
    // SAFETY: called during single-threaded startup, before any other function here.
    unsafe { BINDING.set(Some(binding)) };
}

fn binding() -> Option<&'static dyn Async> {
    // SAFETY: `set_binding` runs before the tasks start; afterwards the cell is only read.
    *unsafe { BINDING.get() }
}

/// Execute `runnable` once on `context`.
pub fn execute(context: ContextType, runnable: &'static dyn Runnable) {
    if let Some(binding) = binding() {
        binding.execute(context, runnable);
    }
}

/// Execute `runnable` on `context` after `delay` units, through `timeout`.
pub fn schedule(
    context: ContextType,
    runnable: &'static dyn Runnable,
    timeout: &'static Timeout,
    delay: u32,
    unit: TimeUnit,
) {
    if let Some(binding) = binding() {
        binding.schedule(context, runnable, timeout, delay, unit);
    }
}

/// Execute `runnable` on `context` every `period` units, through `timeout`.
pub fn schedule_at_fixed_rate(
    context: ContextType,
    runnable: &'static dyn Runnable,
    timeout: &'static Timeout,
    period: u32,
    unit: TimeUnit,
) {
    if let Some(binding) = binding() {
        binding.schedule_at_fixed_rate(context, runnable, timeout, period, unit);
    }
}

/// Cancel `timeout`.
pub fn cancel(timeout: &'static Timeout) {
    if let Some(binding) = binding() {
        binding.cancel(timeout);
    }
}

/// The context of the calling task, `CONTEXT_INVALID` in an interrupt or without a binding.
pub fn current_context() -> ContextType {
    binding().map_or(CONTEXT_INVALID, |binding| binding.current_context())
}

/// The name of the task of `context`, empty without a binding.
pub fn task_name(context: ContextType) -> &'static [u8] {
    binding().map_or(b"", |binding| binding.task_name(context))
}
