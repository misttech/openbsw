// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The console command tree, ported from `util/command`.
//!
//! Commands are `'static` objects linked into a [`ParentCommand`]'s sorted list, as the
//! C++ commands are linked through `etl::forward_link`. A command executes with a
//! [`CommandContext`] that tokenizes the argument string and carries the result.

mod context;
mod group;
mod help;
mod parent;
mod simple;

pub use context::{CommandContext, IdentifierChecker, IntToken};
pub use group::{CommandInfo, GroupCommand};
pub use help::HelpCommand;
pub use parent::ParentCommand;
pub use simple::SimpleCommand;

use crate::cell::RacyCell;
use crate::stream::SharedOutputStream;

/// The result of executing a command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandResult {
    /// No command of that name.
    NotResponsible,
    /// Success.
    Ok,
    /// A token could not be scanned.
    BadToken,
    /// A token where the line should have ended.
    UnexpectedToken,
    /// A token with a value out of range.
    BadValue,
    /// The command failed.
    Error,
}

/// Receives help information: the port of `ICommand::IHelpCallback`.
pub trait HelpCallback {
    /// The help of a command begins. `end` says the command is simple and ends here;
    /// otherwise [`end_command`](Self::end_command) follows the nested commands.
    fn start_command(&mut self, id: &[u8], description: Option<&[u8]>, end: bool);

    /// The end of the information started with `start_command`.
    fn end_command(&mut self);
}

/// The outcome of [`Command::execute`]: the result, the unconsumed rest of the
/// arguments, and the command that ran.
#[derive(Clone, Copy)]
pub struct ExecuteResult<'a> {
    result: CommandResult,
    suffix: &'a [u8],
    command: Option<&'static dyn Command>,
}

impl<'a> ExecuteResult<'a> {
    /// A result with `suffix` left over from `command`.
    pub fn new(
        result: CommandResult,
        suffix: &'a [u8],
        command: Option<&'static dyn Command>,
    ) -> Self {
        Self { result, suffix, command }
    }

    /// Whether the result is `Ok`.
    pub fn is_valid(&self) -> bool {
        self.result == CommandResult::Ok
    }

    /// The result.
    pub fn result(&self) -> CommandResult {
        self.result
    }

    /// The unconsumed rest of the arguments.
    pub fn suffix(&self) -> &'a [u8] {
        self.suffix
    }

    /// The command that ran, if any.
    pub fn command(&self) -> Option<&'static dyn Command> {
        self.command
    }
}

impl core::fmt::Debug for ExecuteResult<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ExecuteResult")
            .field("result", &self.result)
            .field("suffix", &self.suffix)
            .field("command", &self.command.map(|command| command.id()))
            .finish()
    }
}

impl Default for ExecuteResult<'_> {
    fn default() -> Self {
        Self::new(CommandResult::Ok, b"", None)
    }
}

/// The link that puts a command into a parent's list: the port of the
/// `etl::forward_link` base class.
pub struct CommandNode {
    next: RacyCell<Option<&'static dyn Command>>,
}

impl CommandNode {
    /// An unlinked node.
    pub const fn new() -> Self {
        Self { next: RacyCell::new(None) }
    }

    fn next(&self) -> Option<&'static dyn Command> {
        // SAFETY: the list is only changed by `add_command`/`clear_commands` during
        // single-threaded startup, as the C++ console is set up before the tasks run.
        unsafe { *self.next.get() }
    }

    fn set_next(&self, next: Option<&'static dyn Command>) {
        // SAFETY: see `next`; callers change the list only during startup.
        unsafe { self.next.set(next) };
    }
}

impl Default for CommandNode {
    fn default() -> Self {
        Self::new()
    }
}

/// A console command: the port of `ICommand`.
pub trait Command: Sync {
    /// The string that identifies the command.
    fn id(&self) -> &[u8];

    /// Execute the command with `arguments`. Without a stream, only the executing
    /// subcommand is looked up.
    fn execute<'a>(
        &self,
        arguments: &'a [u8],
        shared: Option<&mut dyn SharedOutputStream>,
    ) -> ExecuteResult<'a>;

    /// Report the help information to `callback`.
    fn help(&self, callback: &mut dyn HelpCallback);

    /// The node that links this command into a parent's list.
    fn node(&self) -> &CommandNode;
}
