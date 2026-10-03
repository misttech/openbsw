// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Port of Eclipse OpenBSW's `libs/bsw/logger` and `libs/bsw/loggerIntegration`.
//!
//! Log messages are serialized with their arguments into a ring of entries at log time
//! ([`BufferedLoggerOutput`]), stamped with the time they were logged, and formatted when
//! the application drains them, one per call ([`LoggerComposition::run`]), onto the
//! console as `<ms>: <name>: <component>: <level>: <message>` ([`ConsoleEntryFormatter`],
//! [`ConsoleEntryOutput`]). The [`ComponentMapping`] assigns the component indices and
//! holds each component's level; [`ComponentConfig`] connects it to the `Logger` facade.
//!
//! Not ported: `ILoggerListener`, `DefaultLoggerCommand`, `PersistentComponentConfig`,
//! `SharedStreamEntryOutput`, `DefaultEntryFormatter` and `LoggerTime` (strftime), which
//! the demo does not use.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

mod buffered_output;
mod composition;
mod config;
mod console_output;
mod entry_buffer;
mod entry_serializer;
mod formatter;
mod mapping;
mod time;

#[cfg(test)]
extern crate std;

/// Tests that install the process-wide `Logger` binding run one at a time.
#[cfg(test)]
pub(crate) static LOGGER_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub use buffered_output::{BufferedLoggerOutput, BufferedOutput, EntryOutput};
pub use composition::LoggerComposition;
pub use config::ComponentConfig;
pub use console_output::ConsoleEntryOutput;
pub use entry_buffer::{EntryBuffer, EntryRef};
pub use entry_serializer::{EntryReader, OnEntry, deserialize, serialize};
pub use formatter::{ConsoleEntryFormatter, EntryFormatter};
pub use mapping::{ComponentMapping, MappingInfo};
pub use time::{DefaultLoggerTime, LoggerTime};
