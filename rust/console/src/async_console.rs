// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The command tree and its runner, ported from `console/AsyncConsole.h` and `.cpp`.

use core::cell::Cell;

use openbsw_util::command::{Command, CommandResult, HelpCommand, ParentCommand};
use openbsw_util::format::{Arg, StringWriter, with_shared_writer};
use openbsw_util::log_info;

use crate::CONSOLE;
use crate::output::{ConsoleOutput, ConsoleOutputRef};
use crate::wrappers::SyncCommandWrapper;

/// Called when the console finished with a line: the port of `OnLineProcessed`.
pub trait OnLineProcessed: Sync {
    /// The line was processed; the input may take the next one.
    fn on_line_processed(&self);
}

/// Receives a line of input: the port of `StdioConsoleInput::OnLineReceived`.
pub trait OnLineReceived: Sync {
    /// `line` was entered; answer through `output` and call `on_processed` when done.
    fn on_line_received(
        &self,
        output: &'static ConsoleOutput,
        line: &[u8],
        on_processed: &'static dyn OnLineProcessed,
    );
}

/// The console: a root command holding every registered command, plus `help`.
///
/// A `'static` object: its runners report back to it, and its help command refers to its
/// root (the C++ `singleton_base`).
pub struct AsyncConsole {
    root: ParentCommand,
    help: HelpCommand,
    help_wrapper: SyncCommandWrapper,
    output: Cell<Option<&'static ConsoleOutput>>,
    on_line_processed: Cell<Option<&'static dyn OnLineProcessed>>,
    initialized: Cell<bool>,
}

// SAFETY: the cells are written when a line arrives and read when its command finishes;
// the console input is suspended in between, so there is one line in flight.
unsafe impl Sync for AsyncConsole {}

impl AsyncConsole {
    /// A console, given its own `'static` address.
    pub const fn new(this: &'static AsyncConsole) -> Self {
        Self {
            root: ParentCommand::new(b"root", b"root"),
            help: HelpCommand::new(&this.root, 0),
            help_wrapper: SyncCommandWrapper::new(this, &this.help),
            output: Cell::new(None),
            on_line_processed: Cell::new(None),
            initialized: Cell::new(false),
        }
    }

    /// Register `help`. Call once during startup; the C++ constructor does it.
    pub fn init(&'static self) {
        if !self.initialized.replace(true) {
            self.root.add_command(&self.help_wrapper);
        }
    }

    #[cfg(test)]
    pub(crate) fn init_for_test(&'static self) {
        self.root.add_command(&self.help_wrapper);
    }

    /// Register a command (a wrapper). Call during startup.
    pub fn add_command(&self, command: &'static dyn Command) {
        self.root.add_command(command);
    }

    /// The root command.
    pub fn root(&self) -> &ParentCommand {
        &self.root
    }

    /// The output of the line in flight, for a command that runs later.
    pub fn current_output(&self) -> Option<&'static ConsoleOutput> {
        self.output.get()
    }

    /// A wrapped command finished with `result`: the port of `commandExecuted`.
    pub fn command_executed(&self, result: CommandResult) {
        self.terminate(result);
    }

    fn terminate(&self, result: CommandResult) {
        if let Some(output) = self.output.take() {
            output.with_output(None, &mut |stream| {
                let mut writer = StringWriter::new(stream);
                if result == CommandResult::Ok {
                    log_info!(CONSOLE, b"Console command succeeded");
                    writer.printf(b"ok\n", &[]);
                } else {
                    log_info!(CONSOLE, b"Console command failed");
                    writer.printf(b"error\n", &[]);
                }
            });
        }
        if let Some(on_line_processed) = self.on_line_processed.take() {
            on_line_processed.on_line_processed();
        }
    }
}

impl OnLineReceived for AsyncConsole {
    fn on_line_received(
        &self,
        output: &'static ConsoleOutput,
        line: &[u8],
        on_processed: &'static dyn OnLineProcessed,
    ) {
        log_info!(CONSOLE, b"Received console command \"%.*s\"", line.len(), Arg::Str(Some(line)));
        self.on_line_processed.set(Some(on_processed));
        self.output.set(Some(output));
        let result = self.root.execute(line, Some(&mut ConsoleOutputRef(output)));
        if result.result() == CommandResult::Ok {
            // The wrapper reports back through `command_executed`.
            return;
        }
        log_info!(CONSOLE, b"Console command failed");
        with_shared_writer(&mut ConsoleOutputRef(output), |writer| {
            writer.printf(b"error\n", &[]);
        });
        on_processed.on_line_processed();
    }
}
