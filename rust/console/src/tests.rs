// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The console end to end on the host: typed bytes in, tagged answers out, with the
//! `demo hello` command of `asyncConsole/test` running on a mock async context.

extern crate std;

use std::sync::Mutex;

use openbsw_async::mock::MockAsync;
use openbsw_util::command::{CommandContext, CommandInfo, GroupCommand};
use openbsw_util::format::with_shared_writer;

use crate::output::tests::{FakeStdio, stdio};
use crate::stdio_input::LINE_CAPACITY;
use crate::{AsyncCommandWrapper, AsyncConsole, StdioConsoleInput, SyncCommandWrapper};

/// The console and the async binding are process-wide statics here, so the tests run
/// one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

static MOCK_ASYNC: MockAsync<2> = MockAsync::new([b"main" as &[u8], b"idle"]);
static CONSOLE: AsyncConsole = AsyncConsole::new(&CONSOLE);

// The port of asyncConsole/test's DemoCommand.
static DEMO_INFO: [CommandInfo; 2] =
    [CommandInfo::new(b"demo", b"Demo Commands", 0), CommandInfo::new(b"hello", b"Print hello", 1)];
fn demo_execute(context: &mut CommandContext<'_, '_>, idx: u8) {
    if idx == 1 {
        with_shared_writer(context, |writer| {
            writer.printf(b"Hello World", &[]);
        });
    }
}
static DEMO: GroupCommand = GroupCommand::new(&DEMO_INFO, &demo_execute);
static DEMO_WRAPPER: AsyncCommandWrapper =
    AsyncCommandWrapper::new(&DEMO_WRAPPER, &CONSOLE, &DEMO, 1);

static SYNC_INFO: [CommandInfo; 2] =
    [CommandInfo::new(b"sync", b"Sync Commands", 0), CommandInfo::new(b"now", b"Print now", 1)];
fn sync_execute(context: &mut CommandContext<'_, '_>, idx: u8) {
    if idx == 1 {
        with_shared_writer(context, |writer| {
            writer.printf(b"now\n", &[]);
        });
    }
}
static SYNC: GroupCommand = GroupCommand::new(&SYNC_INFO, &sync_execute);
static SYNC_WRAPPER: SyncCommandWrapper = SyncCommandWrapper::new(&CONSOLE, &SYNC);

fn console() -> &'static FakeStdio {
    openbsw_async::set_binding(&MOCK_ASYNC);
    CONSOLE.init();
    CONSOLE.root().clear_commands();
    CONSOLE.init_for_test();
    CONSOLE.add_command(&DEMO_WRAPPER);
    CONSOLE.add_command(&SYNC_WRAPPER);
    stdio()
}

fn input(stdio: &'static FakeStdio) -> &'static StdioConsoleInput {
    let input: &'static StdioConsoleInput =
        std::boxed::Box::leak(std::boxed::Box::new(StdioConsoleInput::new(stdio, b" ", b"\r\n")));
    input.init(&CONSOLE);
    input
}

fn run_until_idle(input: &'static StdioConsoleInput) {
    for _ in 0..LINE_RUNS {
        input.run();
    }
}
const LINE_RUNS: usize = 200;

#[test]
fn echoes_edits_and_prompts() {
    let _serial = SERIAL.lock().unwrap();
    let stdio = console();
    let input = input(stdio);
    // Printable bytes are echoed; a backspace erases; an empty line prints the prompt.
    stdio.type_in(b"ab\x7fc\r");
    run_until_idle(input);
    // "abc" minus "b": the line is "ac", an unknown command.
    assert_eq!(stdio.take_output(), b"ab\x08 \x08c\r\n error\r\n\r\n> ");
    stdio.type_in(b"\r");
    run_until_idle(input);
    assert_eq!(stdio.take_output(), b"\r\n> ");
    // Escape drops the line.
    stdio.type_in(b"xyz\x1b");
    run_until_idle(input);
    assert_eq!(stdio.take_output(), b"xyz\r\n> ");
    // A byte that is nothing (255) and a NUL are ignored.
    stdio.type_in(b"\x00");
    run_until_idle(input);
    assert_eq!(stdio.take_output(), b"");
}

// The BSP's 255 means no byte is waiting: an idle poll keeps a full line, which the next
// typed byte clears, as the C++ does.
#[test]
fn an_idle_poll_keeps_a_full_line() {
    let _serial = SERIAL.lock().unwrap();
    let stdio = console();
    let input = input(stdio);
    stdio.type_in(&[b'a'; LINE_CAPACITY]);
    run_until_idle(input);
    assert_eq!(input.line_length(), LINE_CAPACITY);
    assert_eq!(stdio.take_output(), [b'a'; LINE_CAPACITY]);
    stdio.type_in(b"b");
    run_until_idle(input);
    assert_eq!(input.line_length(), 1);
    assert_eq!(stdio.take_output(), b"b");
}

#[test]
fn sync_command_answers_at_once() {
    let _serial = SERIAL.lock().unwrap();
    let stdio = console();
    let input = input(stdio);
    stdio.type_in(b"sync now\r");
    run_until_idle(input);
    assert_eq!(stdio.take_output(), b"sync now\r\n now\r\n ok\r\n\r\n> ");
    // help lists everything, through the help command's own sync wrapper.
    stdio.type_in(b"help\r");
    run_until_idle(input);
    let output = stdio.take_output();
    let text = core::str::from_utf8(&output).unwrap();
    assert!(text.starts_with("help\r\n"), "{text:?}");
    assert!(text.contains(" demo    - Demo Commands\r\n   hello - Print hello\r\n"), "{text:?}");
    assert!(text.contains(" help    - Show all commands"), "{text:?}");
    assert!(text.ends_with(" ok\r\n\r\n> "), "{text:?}");
}

#[test]
fn async_command_runs_on_its_context_and_suspends_the_input() {
    let _serial = SERIAL.lock().unwrap();
    let stdio = console();
    let input = input(stdio);
    stdio.type_in(b"demo hello\rnext\r");
    run_until_idle(input);
    // The line is taken, the command queued on context 1, and the input suspended: the
    // bytes after the line wait.
    assert_eq!(stdio.take_output(), b"demo hello\r\n");
    assert!(MOCK_ASYNC.has_runnable(1));
    assert_eq!(stdio.input.borrow().len(), 5);
    MOCK_ASYNC.run_runnables(1);
    assert_eq!(stdio.take_output(), b" Hello World\r\n ok\r\n\r\n> ");
    // The input resumes with the waiting bytes.
    run_until_idle(input);
    assert_eq!(stdio.take_output(), b"next\r\n error\r\n\r\n> ");
    // A subcommand that does not exist is still queued (the wrapper cannot tell), and
    // fails on the context.
    stdio.type_in(b"demo bye\r");
    run_until_idle(input);
    assert_eq!(stdio.take_output(), b"demo bye\r\n");
    MOCK_ASYNC.run_runnables(1);
    assert_eq!(stdio.take_output(), b" error\r\n\r\n> ");
    assert!(!MOCK_ASYNC.has_runnable(1));
}

#[test]
fn a_shut_down_input_drops_lines() {
    let _serial = SERIAL.lock().unwrap();
    let stdio = console();
    let input = input(stdio);
    input.shutdown();
    stdio.type_in(b"sync now\r");
    run_until_idle(input);
    assert_eq!(stdio.take_output(), b"sync now\r\n\r\n> ");
}
