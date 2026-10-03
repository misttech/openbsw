// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The console's output stream: the standard output, shared between the console and the
//! commands, with every line tagged. The port of the `StdoutStream`, `SharedOutputStream`
//! and `TaggedSharedOutputStream` members of `StdioConsoleInput`, fused into one owned,
//! `static`-capable object.

use core::cell::Cell;

use openbsw_util::stream::{ContinuousUser, OutputStream, SharedOutputStream, Stdio};

/// The tagged, shared standard output.
pub struct ConsoleOutput {
    stdio: &'static dyn Stdio,
    prefix: &'static [u8],
    suffix: &'static [u8],
    line_start: Cell<bool>,
    user: Cell<Option<&'static dyn ContinuousUser>>,
}

// SAFETY: the console is suspended while a command runs, so one context at a time writes,
// which is the contract the C++ streams rely on.
unsafe impl Sync for ConsoleOutput {}

/// Writes to the standard output, putting `prefix` at the start of each line and
/// `suffix` at its end: the port of `TaggedOutputHelper` over `StdoutStream`.
struct TaggedStdout<'o> {
    output: &'o ConsoleOutput,
}

impl OutputStream for TaggedStdout<'_> {
    fn is_eof(&self) -> bool {
        false
    }

    fn write(&mut self, data: u8) {
        let output = self.output;
        if output.line_start.get() {
            output.line_start.set(false);
            output.write_raw(output.prefix);
        }
        if data == b'\n' {
            output.write_raw(output.suffix);
            output.line_start.set(true);
        } else {
            output.stdio.put_byte(data);
        }
    }
}

impl ConsoleOutput {
    /// An output over `stdio`, tagging each line with `prefix` and `suffix`.
    pub const fn new(
        stdio: &'static dyn Stdio,
        prefix: &'static [u8],
        suffix: &'static [u8],
    ) -> Self {
        Self { stdio, prefix, suffix, line_start: Cell::new(true), user: Cell::new(None) }
    }

    /// The standard streams.
    pub fn stdio(&self) -> &'static dyn Stdio {
        self.stdio
    }

    /// Run `f` with the stream, as one output of `user` (see
    /// [`SharedOutputStream::with_output`]).
    pub fn with_output(
        &self,
        user: Option<&'static dyn ContinuousUser>,
        f: &mut dyn FnMut(&mut dyn OutputStream),
    ) {
        let previous = self.user.get();
        if user.is_none() || !same_user(previous, user) {
            if let Some(previous) = previous {
                previous.end_continuous_output(&mut TaggedStdout { output: self });
            }
            self.end_line();
        }
        f(&mut TaggedStdout { output: self });
        if user.is_none() {
            self.end_line();
        }
        self.user.set(user);
    }

    /// Forget `user` as the continuous user, ending its line.
    pub fn release_continuous_user(&self, user: &dyn ContinuousUser) {
        if same_user(self.user.get(), Some(user)) {
            self.with_output(None, &mut |_| {});
        }
    }

    /// Write `bytes` straight to the standard output, outside any line tagging.
    pub fn write_raw(&self, bytes: &[u8]) {
        for &byte in bytes {
            self.stdio.put_byte(byte);
        }
    }

    fn end_line(&self) {
        if !self.line_start.get() {
            self.write_raw(self.suffix);
            self.line_start.set(true);
        }
    }
}

fn same_user(a: Option<&dyn ContinuousUser>, b: Option<&dyn ContinuousUser>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => core::ptr::addr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

/// A [`ConsoleOutput`] as the `&mut dyn SharedOutputStream` a command takes.
pub struct ConsoleOutputRef<'o>(pub &'o ConsoleOutput);

impl SharedOutputStream for ConsoleOutputRef<'_> {
    fn with_output(
        &mut self,
        user: Option<&'static dyn ContinuousUser>,
        f: &mut dyn FnMut(&mut dyn OutputStream),
    ) {
        self.0.with_output(user, f);
    }

    fn release_continuous_user(&mut self, user: &dyn ContinuousUser) {
        self.0.release_continuous_user(user);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    extern crate std;

    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::vec::Vec;

    use openbsw_util::format::with_shared_writer;

    use super::*;

    /// The standard streams of a test: bytes to read and the bytes written.
    #[derive(Default)]
    pub(crate) struct FakeStdio {
        pub(crate) input: RefCell<VecDeque<u8>>,
        pub(crate) output: RefCell<Vec<u8>>,
    }
    // SAFETY: single-threaded test object.
    unsafe impl Sync for FakeStdio {}

    impl FakeStdio {
        pub(crate) fn type_in(&self, bytes: &[u8]) {
            self.input.borrow_mut().extend(bytes.iter().copied());
        }
        pub(crate) fn take_output(&self) -> Vec<u8> {
            core::mem::take(&mut *self.output.borrow_mut())
        }
    }

    impl Stdio for FakeStdio {
        fn get_byte(&self) -> i32 {
            self.input.borrow_mut().pop_front().map_or(255, i32::from)
        }
        fn put_byte(&self, byte: u8) {
            self.output.borrow_mut().push(byte);
        }
    }

    pub(crate) fn stdio() -> &'static FakeStdio {
        std::boxed::Box::leak(std::boxed::Box::new(FakeStdio::default()))
    }

    #[test]
    fn tags_each_line() {
        let stdio = stdio();
        let output = ConsoleOutput::new(stdio, b"> ", b"\r\n");
        with_shared_writer(&mut ConsoleOutputRef(&output), |writer| {
            writer.printf(b"a\nb", &[]);
        });
        assert_eq!(stdio.take_output(), b"> a\r\n> b\r\n");
        output.with_output(None, &mut |stream| stream.write_bytes(b"ok\n"));
        assert_eq!(stdio.take_output(), b"> ok\r\n");
        output.write_raw(b"> ");
        assert_eq!(stdio.take_output(), b"> ");
    }

    struct User;
    impl ContinuousUser for User {
        fn end_continuous_output(&self, stream: &mut dyn OutputStream) {
            stream.write_bytes(b"!");
        }
    }
    static USER: User = User;

    #[test]
    fn continuous_user_keeps_its_line() {
        let stdio = stdio();
        let output = ConsoleOutput::new(stdio, b"[", b"]");
        output.with_output(Some(&USER), &mut |stream| stream.write_bytes(b"a"));
        output.with_output(Some(&USER), &mut |stream| stream.write_bytes(b"b"));
        assert_eq!(stdio.take_output(), b"[ab");
        // Another output ends the user's line first.
        output.with_output(None, &mut |stream| stream.write_bytes(b"c"));
        assert_eq!(stdio.take_output(), b"!][c]");
        output.with_output(Some(&USER), &mut |stream| stream.write_bytes(b"d"));
        output.release_continuous_user(&USER);
        assert_eq!(stdio.take_output(), b"[d!]");
    }
}
