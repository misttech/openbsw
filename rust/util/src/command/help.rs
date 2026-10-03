// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The `help` command, ported from `util/command/HelpCommand`.

use super::{Command, CommandContext, CommandNode, CommandResult, ExecuteResult, HelpCallback};
use crate::format::{StringWriter, with_shared_writer};
use crate::stream::SharedOutputStream;

/// Prints the help of a command tree, or of the command named by its arguments.
///
/// Each line is `<indent><id><padding> - <description>`; the id column is as wide as the
/// widest indented id unless a width is given.
pub struct HelpCommand {
    id: &'static [u8],
    description: &'static [u8],
    command: &'static dyn Command,
    id_column_width: u32,
    node: CommandNode,
}

const DEFAULT_ID: &[u8] = b"help";
const DEFAULT_DESCRIPTION: &[u8] =
    b"Show all commands or specific help for a command given as parameter.";

impl HelpCommand {
    /// A `help` command for the tree at `command`; `id_column_width` 0 measures the tree.
    pub const fn new(command: &'static dyn Command, id_column_width: u32) -> Self {
        Self::with_id(command, DEFAULT_ID, DEFAULT_DESCRIPTION, id_column_width)
    }

    /// A help command with its own id and description.
    pub const fn with_id(
        command: &'static dyn Command,
        id: &'static [u8],
        description: &'static [u8],
        id_column_width: u32,
    ) -> Self {
        Self { id, description, command, id_column_width, node: CommandNode::new() }
    }

    /// The description.
    pub fn description(&self) -> &'static [u8] {
        self.description
    }

    fn help(&self, context: &mut CommandContext<'_, '_>) {
        let mut command: Option<&'static dyn Command> = Some(self.command);
        let mut show_first = false;
        while let Some(current) = command
            && context.has_token()
        {
            let token = context.scan_identifier_token();
            let result = current.execute(token, None);
            command = if context.check(result.is_valid(), CommandResult::BadValue) {
                result.command()
            } else {
                None
            };
            show_first = true;
        }
        if let Some(command) = command {
            let id_column_width = self.id_column_width;
            with_shared_writer(context, |writer| {
                CallbackHelper::new(id_column_width, show_first).print_help(command, writer);
            });
        }
    }

    fn as_static(&self) -> &'static dyn Command {
        // SAFETY: commands are `static` items (the tree links them for the program's
        // lifetime), so extending the borrow to `'static` names the same object.
        unsafe { core::mem::transmute::<&dyn Command, &'static dyn Command>(self) }
    }
}

impl Command for HelpCommand {
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
            self.help(&mut context);
            return ExecuteResult::new(context.result(), context.suffix(), Some(self.as_static()));
        }
        ExecuteResult::new(CommandResult::Ok, arguments, Some(self.as_static()))
    }

    fn help(&self, callback: &mut dyn HelpCallback) {
        callback.start_command(self.id, Some(self.description), true);
    }

    fn node(&self) -> &CommandNode {
        &self.node
    }
}

/// Measures the id column, then prints the tree.
struct CallbackHelper<'w, 's> {
    writer: Option<&'w mut StringWriter<'s>>,
    depth: i32,
    id_column_width: u32,
}

impl<'w, 's> CallbackHelper<'w, 's> {
    fn new(id_column_width: u32, show_first: bool) -> Self {
        Self { writer: None, depth: if show_first { 0 } else { -1 }, id_column_width }
    }

    fn print_help(&mut self, command: &dyn Command, writer: &'w mut StringWriter<'s>) {
        if self.id_column_width == 0 {
            self.writer = None;
            command.help(self);
        }
        self.writer = Some(writer);
        command.help(self);
    }

    fn print_description(writer: &mut StringWriter<'_>, id_column_width: u32, description: &[u8]) {
        let mut start = 0;
        let mut current = 0;
        loop {
            let c = description.get(current).copied().unwrap_or(0);
            if c == 0 || is_cr_lf(c) {
                writer.write(&description[start..current]);
                writer.write_char(b'\n');
                while current < description.len() && is_cr_lf(description[current]) {
                    current += 1;
                }
                while current < description.len() && is_whitespace(description[current]) {
                    current += 1;
                }
                if current >= description.len() {
                    break;
                }
                write_spaces(writer, id_column_width + 3);
                start = current;
            } else {
                current += 1;
            }
        }
    }
}

impl HelpCallback for CallbackHelper<'_, '_> {
    fn start_command(&mut self, id: &[u8], description: Option<&[u8]>, end: bool) {
        if self.depth >= 0
            && let Some(description) = description
        {
            let width = (self.depth as u32) * 2 + id.len() as u32;
            match self.writer.as_deref_mut() {
                Some(writer) => {
                    write_spaces(writer, (self.depth as u32) * 2);
                    writer.write(id);
                    write_spaces(writer, self.id_column_width.saturating_sub(width));
                    writer.write(b" - ");
                    Self::print_description(writer, self.id_column_width, description);
                }
                None => self.id_column_width = self.id_column_width.max(width),
            }
        }
        if !end {
            self.depth += 1;
        }
    }

    fn end_command(&mut self) {
        self.depth -= 1;
    }
}

fn write_spaces(writer: &mut StringWriter<'_>, count: u32) {
    for _ in 0..count {
        writer.write_char(b' ');
    }
}

fn is_cr_lf(c: u8) -> bool {
    c == b'\r' || c == b'\n'
}

fn is_whitespace(c: u8) -> bool {
    c == b' ' || c == b'\t'
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{ParentCommand, SimpleCommand};
    use crate::stream::{SharedOutputStreamImpl, StringBufferOutputStream};
    use std::vec::Vec;

    fn dummy(_context: &mut CommandContext<'_, '_>) {}

    static LEAF_1A_2A: SimpleCommand =
        SimpleCommand::new(b"leaf_1a_2a", Some(b"leaf_1a_2a desc."), &dummy);
    static LEAF_1A_2B: SimpleCommand =
        SimpleCommand::new(b"leaf_1a_2b", Some(b"leaf_1a_2b desc."), &dummy);
    static LEAF_1A_2C: SimpleCommand = SimpleCommand::new(b"leaf_1a_2c", None, &dummy);
    static LEVEL_1A: ParentCommand = ParentCommand::new(b"level_1a", b"level_1a desc.");
    static LEAF_1B_2B_3A: SimpleCommand =
        SimpleCommand::new(b"leaf_1b_2b_3a", Some(b"leaf_1b_2b_3a desc."), &dummy);
    static LEVEL_1B_2B: ParentCommand = ParentCommand::new(b"level_1b_2b", b"level_1b_2b desc.");
    static LEAF_1B_2A: SimpleCommand =
        SimpleCommand::new(b"leaf_1b_2a", Some(b"leaf_1b_2a desc."), &dummy);
    static LEAF_1B_2C: SimpleCommand =
        SimpleCommand::new(b"leaf_1b_2c", Some(b"leaf_1b_2c desc."), &dummy);
    static LEVEL_1B: ParentCommand = ParentCommand::new(b"level_1b", b"level_1b desc.");
    static ROOT: ParentCommand = ParentCommand::new(b"root", b"root desc.");

    /// The tests share the tree, so they run one at a time.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn build_tree() -> std::sync::MutexGuard<'static, ()> {
        let guard = SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        ROOT.clear_commands();
        LEVEL_1A.clear_commands();
        LEVEL_1B.clear_commands();
        LEVEL_1B_2B.clear_commands();
        LEVEL_1A.add_command(&LEAF_1A_2A);
        LEVEL_1A.add_command(&LEAF_1A_2B);
        LEVEL_1A.add_command(&LEAF_1A_2C);
        LEVEL_1B_2B.add_command(&LEAF_1B_2B_3A);
        LEVEL_1B.add_command(&LEAF_1B_2A);
        LEVEL_1B.add_command(&LEAF_1B_2C);
        LEVEL_1B.add_command(&LEVEL_1B_2B);
        ROOT.add_command(&LEVEL_1A);
        ROOT.add_command(&LEVEL_1B);
        guard
    }

    fn run(cut: &'static HelpCommand, arguments: &[u8]) -> (Vec<u8>, CommandResult, Vec<u8>) {
        let mut buffer = [0u8; 300];
        let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
        let (result, suffix) = {
            let mut shared = SharedOutputStreamImpl::new(&mut stream);
            let result = cut.execute(arguments, Some(&mut shared));
            (result.result(), result.suffix().to_vec())
        };
        (stream.string().to_vec(), result, suffix)
    }

    // Ported from util/test/src/util/command/HelpCommandTest.cpp (testAll). The tree is
    // built once per test since the statics are shared.
    #[test]
    fn ids_and_descriptions() {
        static CUT: HelpCommand = HelpCommand::new(&ROOT, 0);
        assert_eq!(CUT.id(), b"help");
        static NAMED: HelpCommand = HelpCommand::with_id(&ROOT, b"id", b"description", 0);
        assert_eq!(NAMED.id(), b"id");
        assert_eq!(NAMED.description(), b"description");
        static PARENT: ParentCommand = ParentCommand::new(b"parent", b"desc");
        PARENT.add_command(&CUT);
        assert!(PARENT.commands().next().is_some());
    }

    #[test]
    fn prints_the_whole_tree() {
        let _tree = build_tree();
        static CUT: HelpCommand = HelpCommand::new(&ROOT, 0);
        let (output, result, _) = run(&CUT, b"");
        assert_eq!(result, CommandResult::Ok);
        assert_eq!(
            output,
            b"level_1a          - level_1a desc.\n\
              \x20 leaf_1a_2a      - leaf_1a_2a desc.\n\
              \x20 leaf_1a_2b      - leaf_1a_2b desc.\n\
              level_1b          - level_1b desc.\n\
              \x20 leaf_1b_2a      - leaf_1b_2a desc.\n\
              \x20 leaf_1b_2c      - leaf_1b_2c desc.\n\
              \x20 level_1b_2b     - level_1b_2b desc.\n\
              \x20   leaf_1b_2b_3a - leaf_1b_2b_3a desc.\n"
        );
    }

    #[test]
    fn prints_a_subtree() {
        let _tree = build_tree();
        static CUT: HelpCommand = HelpCommand::new(&ROOT, 0);
        let (output, result, _) = run(&CUT, b"level_1b");
        assert_eq!(result, CommandResult::Ok);
        assert_eq!(
            output,
            b"level_1b          - level_1b desc.\n\
              \x20 leaf_1b_2a      - leaf_1b_2a desc.\n\
              \x20 leaf_1b_2c      - leaf_1b_2c desc.\n\
              \x20 level_1b_2b     - level_1b_2b desc.\n\
              \x20   leaf_1b_2b_3a - leaf_1b_2b_3a desc.\n"
        );
    }

    #[test]
    fn fixed_column_width() {
        let _tree = build_tree();
        static CUT: HelpCommand = HelpCommand::new(&ROOT, 18);
        let (output, result, _) = run(&CUT, b"level_1b");
        assert_eq!(result, CommandResult::Ok);
        assert_eq!(
            output,
            b"level_1b           - level_1b desc.\n\
              \x20 leaf_1b_2a       - leaf_1b_2a desc.\n\
              \x20 leaf_1b_2c       - leaf_1b_2c desc.\n\
              \x20 level_1b_2b      - level_1b_2b desc.\n\
              \x20   leaf_1b_2b_3a  - leaf_1b_2b_3a desc.\n"
        );
    }

    #[test]
    fn multiline_descriptions() {
        static LEAF1: SimpleCommand =
            SimpleCommand::new(b"leaf1", Some(b"leaf description\r \t also multiline"), &dummy);
        static PARENT: ParentCommand =
            ParentCommand::new(b"parent", b"parent description\nmultiline\n");
        static ROOT2: ParentCommand = ParentCommand::new(b"root", b"root desc.");
        ROOT2.add_command(&PARENT);
        PARENT.add_command(&LEAF1);
        static CUT: HelpCommand = HelpCommand::new(&ROOT2, 0);
        let (output, result, _) = run(&CUT, b"");
        assert_eq!(result, CommandResult::Ok);
        assert_eq!(
            output,
            b"parent  - parent description\n\
              \x20         multiline\n\
              \x20 leaf1 - leaf description\n\
              \x20         also multiline\n"
        );
    }

    #[test]
    fn unknown_subcommand_is_a_bad_value() {
        let _tree = build_tree();
        static CUT: HelpCommand = HelpCommand::new(&ROOT, 0);
        let (_, result, suffix) = run(&CUT, b"level_1a abc");
        assert_eq!(result, CommandResult::BadValue);
        assert_eq!(suffix, b"abc");
    }
}
