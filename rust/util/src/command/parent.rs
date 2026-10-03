// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! A command holding other commands, ported from `util/command/ParentCommand`.

use super::{Command, CommandContext, CommandNode, CommandResult, ExecuteResult, HelpCallback};
use crate::cell::RacyCell;
use crate::stream::SharedOutputStream;
use crate::string::compare_ignore_case;

/// A command that dispatches to its subcommands by name, kept sorted case-insensitively.
///
/// Subcommands are added during startup, before the console runs; the list is read-only
/// afterwards.
pub struct ParentCommand {
    id: &'static [u8],
    description: &'static [u8],
    first: RacyCell<Option<&'static dyn Command>>,
    node: CommandNode,
}

/// The subcommands of a parent, in order.
pub struct Commands {
    next: Option<&'static dyn Command>,
}

impl Iterator for Commands {
    type Item = &'static dyn Command;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.next?;
        self.next = current.node().next();
        Some(current)
    }
}

impl ParentCommand {
    /// A parent `id` described by `description`, with no subcommands.
    pub const fn new(id: &'static [u8], description: &'static [u8]) -> Self {
        Self { id, description, first: RacyCell::new(None), node: CommandNode::new() }
    }

    /// The description.
    pub fn description(&self) -> &'static [u8] {
        self.description
    }

    fn first(&self) -> Option<&'static dyn Command> {
        // SAFETY: the list changes only during single-threaded startup (see the type
        // documentation), so no read overlaps a write.
        unsafe { *self.first.get() }
    }

    fn set_first(&self, first: Option<&'static dyn Command>) {
        // SAFETY: see `first`.
        unsafe { self.first.set(first) };
    }

    /// The subcommands, sorted.
    pub fn commands(&self) -> Commands {
        Commands { next: self.first() }
    }

    /// Add a subcommand at its sorted position. Call during startup only.
    pub fn add_command(&self, command: &'static dyn Command) {
        let id = command.id();
        let mut previous: Option<&'static dyn Command> = None;
        let mut current = self.first();
        while let Some(candidate) = current {
            if compare_ignore_case(id, candidate.id()) <= 0 {
                break;
            }
            previous = Some(candidate);
            current = candidate.node().next();
        }
        command.node().set_next(current);
        match previous {
            None => self.set_first(Some(command)),
            Some(previous) => previous.node().set_next(Some(command)),
        }
    }

    /// Remove all subcommands. Call during startup only.
    pub fn clear_commands(&self) {
        self.set_first(None);
    }

    fn lookup_command(&self, id: &[u8]) -> Option<&'static dyn Command> {
        self.commands().find(|command| compare_ignore_case(command.id(), id) == 0)
    }
}

impl Command for ParentCommand {
    fn id(&self) -> &[u8] {
        self.id
    }

    fn execute<'a>(
        &self,
        arguments: &'a [u8],
        shared: Option<&mut dyn SharedOutputStream>,
    ) -> ExecuteResult<'a> {
        let mut context = CommandContext::new(arguments, None);
        let id = context.scan_identifier_token();
        if let Some(command) = self.lookup_command(id) {
            if let Some(shared) = shared {
                return command.execute(context.suffix(), Some(shared));
            }
            return ExecuteResult::new(CommandResult::Ok, context.suffix(), Some(command));
        }
        ExecuteResult::new(CommandResult::NotResponsible, arguments, None)
    }

    fn help(&self, callback: &mut dyn HelpCallback) {
        callback.start_command(self.id, Some(self.description), false);
        for command in self.commands() {
            command.help(callback);
        }
        callback.end_command();
    }

    fn node(&self) -> &CommandNode {
        &self.node
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::SimpleCommand;
    use crate::command::simple::tests::HelpRecorder;
    use crate::format::with_shared_writer;
    use crate::stream::{SharedOutputStreamImpl, StringBufferOutputStream};

    fn static_func(context: &mut CommandContext<'_, '_>) {
        let value = context.scan_int_token::<i32>();
        with_shared_writer(context, |writer| {
            writer.printf(b"staticFunc: %d", &[value.into()]);
        });
    }

    fn member_func(context: &mut CommandContext<'_, '_>) {
        let value = context.scan_int_token::<i32>();
        with_shared_writer(context, |writer| {
            writer.printf(b"memberFunc: %d", &[value.into()]);
        });
    }

    static STATIC_COMMAND: SimpleCommand =
        SimpleCommand::new(b"static", Some(b"Static Desc."), &static_func);
    static MEMBER_COMMAND: SimpleCommand =
        SimpleCommand::new(b"member", Some(b"Member Desc."), &member_func);
    static OTHER_COMMAND: SimpleCommand =
        SimpleCommand::new(b"other", Some(b"Other Desc."), &static_func);

    // Ported from util/test/src/util/command/ParentCommandTest.cpp.
    #[test]
    fn initialization() {
        static CUT: ParentCommand = ParentCommand::new(b"id", b"description");
        assert_eq!(CUT.id(), b"id");
        assert_eq!(CUT.description(), b"description");
        let mut recorder = HelpRecorder::default();
        CUT.help(&mut recorder);
        assert_eq!(recorder.0, "start: id=id, desc=description, end=0\nend\n");
    }

    #[test]
    fn execution_and_lookup() {
        static CUT: ParentCommand = ParentCommand::new(b"id", b"description");
        CUT.add_command(&STATIC_COMMAND);
        CUT.add_command(&MEMBER_COMMAND);
        {
            let mut buffer = [0u8; 20];
            let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
            {
                let mut shared = SharedOutputStreamImpl::new(&mut stream);
                let result = CUT.execute(b"Static 123", Some(&mut shared));
                assert!(result.is_valid());
                assert_eq!(result.suffix(), b"");
                assert!(
                    result
                        .command()
                        .is_some_and(|c| core::ptr::addr_eq(c, &STATIC_COMMAND as &dyn Command))
                );
            }
            assert_eq!(stream.string(), b"staticFunc: 123");
        }
        {
            let mut buffer = [0u8; 20];
            let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
            {
                let mut shared = SharedOutputStreamImpl::new(&mut stream);
                let result = CUT.execute(b" memBer 123a", Some(&mut shared));
                assert!(!result.is_valid());
                assert_eq!(result.result(), CommandResult::BadToken);
                assert_eq!(result.suffix(), b"a");
                assert!(
                    result
                        .command()
                        .is_some_and(|c| core::ptr::addr_eq(c, &MEMBER_COMMAND as &dyn Command))
                );
            }
            assert_eq!(stream.string(), b"memberFunc: 0");
        }
        {
            let mut buffer = [0u8; 20];
            let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
            {
                let mut shared = SharedOutputStreamImpl::new(&mut stream);
                let result = CUT.execute(b" other 123", Some(&mut shared));
                assert!(!result.is_valid());
                assert_eq!(result.result(), CommandResult::NotResponsible);
                assert_eq!(result.suffix(), b" other 123");
                assert!(result.command().is_none());
            }
            assert_eq!(stream.string(), b"");
        }
        {
            let result = CUT.execute(b" member 123", None);
            assert!(result.is_valid());
            assert_eq!(result.suffix(), b"123");
            assert!(
                result
                    .command()
                    .is_some_and(|c| core::ptr::addr_eq(c, &MEMBER_COMMAND as &dyn Command))
            );
        }
    }

    #[test]
    fn ordered_access() {
        static CUT: ParentCommand = ParentCommand::new(b"id", b"description");
        static STATIC: SimpleCommand =
            SimpleCommand::new(b"static", Some(b"Static Desc."), &static_func);
        static MEMBER: SimpleCommand =
            SimpleCommand::new(b"member", Some(b"Member Desc."), &member_func);
        static OTHER: SimpleCommand =
            SimpleCommand::new(b"other", Some(b"Other Desc."), &static_func);
        CUT.add_command(&STATIC);
        CUT.add_command(&MEMBER);
        CUT.add_command(&OTHER);
        let mut recorder = HelpRecorder::default();
        CUT.help(&mut recorder);
        assert_eq!(
            recorder.0,
            "start: id=id, desc=description, end=0\n\
             start: id=member, desc=Member Desc., end=1\n\
             start: id=other, desc=Other Desc., end=1\n\
             start: id=static, desc=Static Desc., end=1\n\
             end\n"
        );
    }

    #[test]
    fn clear_commands() {
        static CUT: ParentCommand = ParentCommand::new(b"id", b"description");
        static STATIC: SimpleCommand =
            SimpleCommand::new(b"static", Some(b"Static Desc."), &static_func);
        CUT.add_command(&STATIC);
        CUT.add_command(&OTHER_COMMAND);
        CUT.clear_commands();
        let mut recorder = HelpRecorder::default();
        CUT.help(&mut recorder);
        assert_eq!(recorder.0, "start: id=id, desc=description, end=0\nend\n");
    }
}
