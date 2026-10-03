// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Port of `libs/bsw/asyncConsole` and `libs/bsw/stdioConsoleInput`: the command console.
//!
//! [`StdioConsoleInput`] collects a line from the standard input, echoing and editing it,
//! and hands it to [`AsyncConsole`], which looks the command up in its tree and runs it:
//! at once through a [`SyncCommandWrapper`], or on an async context through an
//! [`AsyncCommandWrapper`]. The command's output goes through the console's
//! [`ConsoleOutput`], each line tagged with a prefix and a suffix, and `ok` or `error`
//! ends it; the input then prints its prompt again.
//!
//! One difference from the C++: a wrapper does not register itself with the console when
//! it is constructed (a `const fn` cannot), so the application adds each wrapper with
//! [`AsyncConsole::add_command`] at startup, as it adds its lifecycle components.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

mod async_console;
mod output;
mod stdio_input;
#[cfg(test)]
mod tests;
mod wrappers;

use openbsw_util::logger::LoggerComponent;

pub use async_console::{AsyncConsole, OnLineProcessed, OnLineReceived};
pub use output::{ConsoleOutput, ConsoleOutputRef};
pub use stdio_input::{LINE_CAPACITY, StdioConsoleInput};
pub use wrappers::{AsyncCommandWrapper, SyncCommandWrapper};

/// The `CONSOLE` logger component (`logger/ConsoleLogger.h`).
pub static CONSOLE: LoggerComponent = LoggerComponent::new();
