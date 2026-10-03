// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! A command with a fixed table of subcommands, ported from `util/command/GroupCommand`.

use super::{Command, CommandContext, CommandNode, CommandResult, ExecuteResult, HelpCallback};
use crate::stream::SharedOutputStream;
use crate::string::compare_ignore_case;

/// One entry of a group's table: the port of `GroupCommand::PlainCommandInfo`.
#[derive(Clone, Copy, Debug)]
pub struct CommandInfo {
    /// The subcommand's id.
    pub id: &'static [u8],
    /// Its description.
    pub description: &'static [u8],
    /// The index passed to the execute function.
    pub idx: u8,
}

impl CommandInfo {
    /// An entry.
    pub const fn new(id: &'static [u8], description: &'static [u8], idx: u8) -> Self {
        Self { id, description, idx }
    }
}

/// The function a [`GroupCommand`] runs for a subcommand's index.
pub type GroupExecuteFunction = dyn Fn(&mut CommandContext<'_, '_>, u8) + Sync;

/// A command whose subcommands come from a table; the first entry describes the group
/// itself.
pub struct GroupCommand {
    info: &'static [CommandInfo],
    execute: &'static GroupExecuteFunction,
    node: CommandNode,
}

impl GroupCommand {
    /// A group described by `info` (first entry: the group) that runs `execute`.
    pub const fn new(info: &'static [CommandInfo], execute: &'static GroupExecuteFunction) -> Self {
        Self { info, execute, node: CommandNode::new() }
    }

    /// The group's description.
    pub fn description(&self) -> &'static [u8] {
        self.info[0].description
    }

    fn lookup_command(&self, id: &[u8]) -> Option<&'static CommandInfo> {
        self.info.iter().skip(1).find(|info| compare_ignore_case(info.id, id) == 0)
    }

    fn as_static(&self) -> &'static dyn Command {
        // SAFETY: commands are `static` items (the tree links them for the program's
        // lifetime), so extending the borrow to `'static` names the same object.
        unsafe { core::mem::transmute::<&dyn Command, &'static dyn Command>(self) }
    }
}

impl Command for GroupCommand {
    fn id(&self) -> &[u8] {
        self.info[0].id
    }

    fn execute<'a>(
        &self,
        arguments: &'a [u8],
        shared: Option<&mut dyn SharedOutputStream>,
    ) -> ExecuteResult<'a> {
        match shared {
            Some(shared) => {
                let mut context = CommandContext::new(arguments, Some(shared));
                let id = context.scan_token();
                match self.lookup_command(id) {
                    Some(info) => {
                        (self.execute)(&mut context, info.idx);
                        ExecuteResult::new(
                            context.result(),
                            context.suffix(),
                            Some(self.as_static()),
                        )
                    }
                    None => ExecuteResult::new(CommandResult::NotResponsible, arguments, None),
                }
            }
            None => {
                let mut context = CommandContext::new(arguments, None);
                let id = context.scan_token();
                if self.lookup_command(id).is_some() {
                    ExecuteResult::new(CommandResult::Ok, arguments, Some(self.as_static()))
                } else {
                    ExecuteResult::new(CommandResult::NotResponsible, arguments, None)
                }
            }
        }
    }

    fn help(&self, callback: &mut dyn HelpCallback) {
        callback.start_command(self.id(), Some(self.description()), false);
        for info in self.info.iter().skip(1) {
            callback.start_command(info.id, Some(info.description), true);
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
    use crate::command::simple::tests::HelpRecorder;
    use crate::format::with_shared_writer;
    use crate::stream::{SharedOutputStreamImpl, StringBufferOutputStream};

    static INFO: [CommandInfo; 4] = [
        CommandInfo::new(b"group", b"group desc.", 0),
        CommandInfo::new(b"cmd2", b"desc 2", 2),
        CommandInfo::new(b"cmd5", b"desc 5", 5),
        CommandInfo::new(b"675", b"desc 7", 7),
    ];

    fn execute(context: &mut CommandContext<'_, '_>, idx: u8) {
        with_shared_writer(context, |writer| {
            writer.printf(b"cmd = %d\n", &[idx.into()]);
        });
        while context.has_token() {
            let value = context.scan_int_token::<u32>();
            with_shared_writer(context, |writer| {
                writer.printf(b"arg = %d\n", &[value.into()]);
            });
        }
    }

    static CUT: GroupCommand = GroupCommand::new(&INFO, &execute);

    // Ported from util/test/src/util/command/GroupCommandTest.cpp.
    #[test]
    fn initialization() {
        assert_eq!(CUT.id(), b"group");
        assert_eq!(CUT.description(), b"group desc.");
        let mut recorder = HelpRecorder::default();
        CUT.help(&mut recorder);
        assert_eq!(
            recorder.0,
            "start: id=group, desc=group desc., end=0\n\
             start: id=cmd2, desc=desc 2, end=1\n\
             start: id=cmd5, desc=desc 5, end=1\n\
             start: id=675, desc=desc 7, end=1\n\
             end\n"
        );
    }

    #[test]
    fn execution_and_lookup() {
        {
            let mut buffer = [0u8; 100];
            let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
            {
                let mut shared = SharedOutputStreamImpl::new(&mut stream);
                let result = CUT.execute(b" Cmd2 1 2", Some(&mut shared));
                assert!(result.is_valid());
                assert_eq!(result.suffix(), b"");
                assert!(
                    result.command().is_some_and(|c| core::ptr::addr_eq(c, &CUT as &dyn Command))
                );
            }
            assert_eq!(stream.string(), b"cmd = 2\narg = 1\narg = 2\n");
        }
        {
            let mut buffer = [0u8; 100];
            let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
            {
                let mut shared = SharedOutputStreamImpl::new(&mut stream);
                let result = CUT.execute(b" 675 1 2", Some(&mut shared));
                assert!(result.is_valid());
                assert_eq!(result.suffix(), b"");
            }
            assert_eq!(stream.string(), b"cmd = 7\narg = 1\narg = 2\n");
        }
        {
            let result = CUT.execute(b" Cmd2 1 2", None);
            assert!(result.is_valid());
            assert_eq!(result.suffix(), b" Cmd2 1 2");
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
            }
            assert_eq!(stream.string(), b"");
        }
    }
}
