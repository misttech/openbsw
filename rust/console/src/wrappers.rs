// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The runners that put a command into the console's tree, ported from
//! `console/SyncCommandWrapper.h`, `AsyncCommandWrapper.h` and their `.cpp`.

use core::cell::Cell;

use openbsw_async::{ContextType, QueueNode, Runnable};
use openbsw_util::command::{Command, CommandNode, CommandResult, ExecuteResult, HelpCallback};
use openbsw_util::stream::SharedOutputStream;

use crate::async_console::AsyncConsole;
use crate::output::ConsoleOutputRef;
use crate::stdio_input::LINE_CAPACITY;

/// Runs its command at once, on the console's context, and reports the result.
pub struct SyncCommandWrapper {
    console: &'static AsyncConsole,
    command: &'static dyn Command,
    node: CommandNode,
}

impl SyncCommandWrapper {
    /// A wrapper of `command` reporting to `console`.
    pub const fn new(console: &'static AsyncConsole, command: &'static dyn Command) -> Self {
        Self { console, command, node: CommandNode::new() }
    }
}

impl Command for SyncCommandWrapper {
    fn id(&self) -> &[u8] {
        self.command.id()
    }

    fn execute<'a>(
        &self,
        arguments: &'a [u8],
        shared: Option<&mut dyn SharedOutputStream>,
    ) -> ExecuteResult<'a> {
        let result = self.command.execute(arguments, shared);
        self.console.command_executed(result.result());
        ExecuteResult::new(CommandResult::Ok, b"", None)
    }

    fn help(&self, callback: &mut dyn HelpCallback) {
        self.command.help(callback);
    }

    fn node(&self) -> &CommandNode {
        &self.node
    }
}

/// Runs its command on an async context and reports the result from there.
///
/// The arguments are copied, since the console's line buffer is only borrowed for the
/// call (the C++ keeps a `ConstString` into it).
pub struct AsyncCommandWrapper {
    this: &'static AsyncCommandWrapper,
    console: &'static AsyncConsole,
    command: &'static dyn Command,
    context: ContextType,
    arguments: [Cell<u8>; LINE_CAPACITY],
    argument_length: Cell<usize>,
    command_node: CommandNode,
    runnable_node: QueueNode<dyn Runnable>,
}

// SAFETY: the arguments are written when the line arrives and read when the command runs;
// the console has one line in flight.
unsafe impl Sync for AsyncCommandWrapper {}

impl AsyncCommandWrapper {
    /// A wrapper of `command` running it on `context` and reporting to `console`; `this`
    /// is the wrapper's own `'static` address, which it hands to the async binding.
    pub const fn new(
        this: &'static AsyncCommandWrapper,
        console: &'static AsyncConsole,
        command: &'static dyn Command,
        context: ContextType,
    ) -> Self {
        Self {
            this,
            console,
            command,
            context,
            arguments: [const { Cell::new(0) }; LINE_CAPACITY],
            argument_length: Cell::new(0),
            command_node: CommandNode::new(),
            runnable_node: QueueNode::new(),
        }
    }
}

impl Command for AsyncCommandWrapper {
    fn id(&self) -> &[u8] {
        self.command.id()
    }

    /// Queues the command; the result comes through the console when it ran.
    fn execute<'a>(
        &self,
        arguments: &'a [u8],
        _shared: Option<&mut dyn SharedOutputStream>,
    ) -> ExecuteResult<'a> {
        let length = arguments.len().min(LINE_CAPACITY);
        for (cell, &byte) in self.arguments.iter().zip(arguments) {
            cell.set(byte);
        }
        self.argument_length.set(length);
        openbsw_async::execute(self.context, self.this);
        ExecuteResult::new(CommandResult::Ok, b"", None)
    }

    fn help(&self, callback: &mut dyn HelpCallback) {
        self.command.help(callback);
    }

    fn node(&self) -> &CommandNode {
        &self.command_node
    }
}

impl Runnable for AsyncCommandWrapper {
    fn execute(&self) {
        let mut arguments = [0_u8; LINE_CAPACITY];
        let length = self.argument_length.get();
        for (byte, cell) in arguments.iter_mut().zip(self.arguments.iter()) {
            *byte = cell.get();
        }
        let result = match self.console.current_output() {
            Some(output) => {
                self.command.execute(&arguments[..length], Some(&mut ConsoleOutputRef(output)))
            }
            None => self.command.execute(&arguments[..length], None),
        };
        self.console.command_executed(result.result());
    }

    fn node(&self) -> &QueueNode<dyn Runnable> {
        &self.runnable_node
    }
}
