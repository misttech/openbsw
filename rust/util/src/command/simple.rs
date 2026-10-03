// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! A leaf command, ported from `util/command/SimpleCommand`.

use super::{Command, CommandContext, CommandNode, CommandResult, ExecuteResult, HelpCallback};
use crate::stream::SharedOutputStream;

/// The function a [`SimpleCommand`] runs.
pub type ExecuteFunction = dyn Fn(&mut CommandContext<'_, '_>) + Sync;

/// A command with an id, a description, and a function that executes it.
pub struct SimpleCommand {
    id: &'static [u8],
    description: Option<&'static [u8]>,
    execute: &'static ExecuteFunction,
    node: CommandNode,
}

impl SimpleCommand {
    /// A command `id` described by `description` that runs `execute`.
    pub const fn new(
        id: &'static [u8],
        description: Option<&'static [u8]>,
        execute: &'static ExecuteFunction,
    ) -> Self {
        Self { id, description, execute, node: CommandNode::new() }
    }

    /// The description.
    pub fn description(&self) -> Option<&'static [u8]> {
        self.description
    }
}

impl Command for SimpleCommand {
    fn id(&self) -> &[u8] {
        self.id
    }

    fn execute<'a>(
        &self,
        arguments: &'a [u8],
        shared: Option<&mut dyn SharedOutputStream>,
    ) -> ExecuteResult<'a> {
        if let Some(shared) = shared {
            let mut context = CommandContext::new(arguments, Some(shared));
            (self.execute)(&mut context);
            return ExecuteResult::new(context.result(), context.suffix(), Some(self.as_static()));
        }
        ExecuteResult::new(CommandResult::Ok, arguments, Some(self.as_static()))
    }

    fn help(&self, callback: &mut dyn HelpCallback) {
        callback.start_command(self.id, self.description, true);
    }

    fn node(&self) -> &CommandNode {
        &self.node
    }
}

impl SimpleCommand {
    /// This command as the `'static` object it must be to sit in a command tree.
    fn as_static(&self) -> &'static dyn Command {
        // SAFETY: commands are `static` items (the tree links them for the program's
        // lifetime), so extending the borrow to `'static` names the same object.
        unsafe { core::mem::transmute::<&dyn Command, &'static dyn Command>(self) }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::format::with_shared_writer;
    use crate::stream::{SharedOutputStreamImpl, StringBufferOutputStream};
    use std::string::String;
    use std::vec::Vec;

    #[derive(Default)]
    pub(crate) struct HelpRecorder(pub(crate) String);

    impl HelpCallback for HelpRecorder {
        fn start_command(&mut self, id: &[u8], description: Option<&[u8]>, end: bool) {
            self.0.push_str(&std::format!(
                "start: id={}, desc={}, end={}\n",
                String::from_utf8_lossy(id),
                String::from_utf8_lossy(description.unwrap_or(b"(null)")),
                u8::from(end)
            ));
        }

        fn end_command(&mut self) {
            self.0.push_str("end\n");
        }
    }

    fn static_func(context: &mut CommandContext<'_, '_>) {
        let value = context.scan_int_token::<i32>();
        with_shared_writer(context, |writer| {
            writer.printf(b"staticFunc: %d", &[value.into()]);
        });
    }

    static CUT: SimpleCommand = SimpleCommand::new(b"id", Some(b"description"), &static_func);

    fn run(
        command: &'static dyn Command,
        arguments: &[u8],
    ) -> (Vec<u8>, CommandResult, Vec<u8>, bool) {
        let mut buffer = [0u8; 20];
        let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
        let (result, suffix, same) = {
            let mut shared = SharedOutputStreamImpl::new(&mut stream);
            let result = command.execute(arguments, Some(&mut shared));
            (
                result.result(),
                result.suffix().to_vec(),
                result.command().is_some_and(|c| core::ptr::addr_eq(c, command)),
            )
        };
        (stream.string().to_vec(), result, suffix, same)
    }

    // Ported from util/test/src/util/command/SimpleCommandTest.cpp.
    #[test]
    fn initialization_and_execution() {
        assert_eq!(CUT.id(), b"id");
        assert_eq!(CUT.description(), Some(&b"description"[..]));
        let mut recorder = HelpRecorder::default();
        CUT.help(&mut recorder);
        assert_eq!(recorder.0, "start: id=id, desc=description, end=1\n");
        let (output, result, suffix, same) = run(&CUT, b"1235");
        assert_eq!(result, CommandResult::Ok);
        assert_eq!(suffix, b"");
        assert!(same);
        assert_eq!(output, b"staticFunc: 1235");
    }

    #[test]
    fn lookup() {
        let result = CUT.execute(b"1235", None);
        assert!(result.is_valid());
        assert_eq!(result.result(), CommandResult::Ok);
        assert_eq!(result.suffix(), b"1235");
        assert!(result.command().is_some_and(|c| core::ptr::addr_eq(c, &CUT as &dyn Command)));
    }

    #[test]
    fn error_handling() {
        let (output, result, suffix, same) = run(&CUT, b"  123a");
        assert_eq!(result, CommandResult::BadToken);
        assert_eq!(suffix, b"a");
        assert!(same);
        assert_eq!(output, b"staticFunc: 0");
    }
}
