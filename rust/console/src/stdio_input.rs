// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Line input from the standard streams, ported from `console/StdioConsoleInput.h` and
//! `.cpp`: echo, backspace, escape, and a prompt.

use core::cell::Cell;

use openbsw_util::stream::Stdio;

use crate::async_console::{OnLineProcessed, OnLineReceived};
use crate::output::ConsoleOutput;

/// The longest line (`etl::string<128>`); a longer one starts over.
pub const LINE_CAPACITY: usize = 128;

/// What the BSP's `get_byte` returns when no byte is waiting, as an unsigned byte.
const NO_BYTE: u8 = 0xFF;

const BACK_SPACE: u8 = 0x08;
const BACK_SPACE_7F: u8 = 0x7F;
const LINE_FEED: u8 = 0x0A;
const CARRIAGE_RETURN: u8 = 0x0D;
const ESCAPE: u8 = 0x1B;
const MIN_VALID_CHR: u8 = 0x20;
const MAX_VALID_CHR: u8 = 0x7E;

/// Collects lines from the standard input and hands them to a receiver.
pub struct StdioConsoleInput {
    output: ConsoleOutput,
    line: [Cell<u8>; LINE_CAPACITY],
    line_length: Cell<usize>,
    on_line_received: Cell<Option<&'static dyn OnLineReceived>>,
    is_suspended: Cell<bool>,
}

// SAFETY: `run` is called from one context; the receiver's completion callback clears
// the line from whichever context finished the command, while the input is suspended.
unsafe impl Sync for StdioConsoleInput {}

impl StdioConsoleInput {
    /// An input over `stdio`, whose output tags each line with `prefix` and `suffix`.
    pub const fn new(
        stdio: &'static dyn Stdio,
        prefix: &'static [u8],
        suffix: &'static [u8],
    ) -> Self {
        Self {
            output: ConsoleOutput::new(stdio, prefix, suffix),
            line: [const { Cell::new(0) }; LINE_CAPACITY],
            line_length: Cell::new(0),
            on_line_received: Cell::new(None),
            is_suspended: Cell::new(false),
        }
    }

    /// The output the receiver answers through.
    pub fn output(&self) -> &ConsoleOutput {
        &self.output
    }

    /// Hand lines to `on_line_received`.
    pub fn init(&self, on_line_received: &'static dyn OnLineReceived) {
        self.on_line_received.set(Some(on_line_received));
    }

    /// Stop handing lines out; a line is then just dropped.
    pub fn shutdown(&self) {
        self.on_line_received.set(None);
    }

    /// Read one byte, if any, and hand the line out once it is complete.
    pub fn run(&'static self) {
        if self.is_suspended.get() || !self.getline() {
            return;
        }
        if self.line_length.get() > 0 {
            self.print_newline();
            self.is_suspended.set(true);
            match self.on_line_received.get() {
                Some(receiver) => {
                    let mut line = [0_u8; LINE_CAPACITY];
                    let length = self.line_length.get();
                    for (byte, cell) in line.iter_mut().zip(self.line.iter()) {
                        *byte = cell.get();
                    }
                    receiver.on_line_received(&self.output, &line[..length], self);
                }
                // The C++ default receiver only calls back.
                None => self.on_line_processed(),
            }
        } else {
            self.print_prompt();
        }
    }

    /// Take one byte; true when the line is complete.
    ///
    /// A negative value, 255 or a NUL means nothing was waiting. The C++ casts the byte to
    /// `char` and stops at `chr <= 0`, which catches the BSP's 255 where `char` is signed;
    /// the Zephyr BSP stores `-1` in an `unsigned char`, so 255 is "nothing" here too,
    /// and an idle poll leaves a full line alone.
    fn getline(&self) -> bool {
        let Ok(chr) = u8::try_from(self.output.stdio().get_byte()) else {
            return false;
        };
        if chr == 0 || chr == NO_BYTE {
            return false;
        }
        if self.line_length.get() == LINE_CAPACITY {
            self.line_length.set(0);
        }
        if (MIN_VALID_CHR..=MAX_VALID_CHR).contains(&chr) {
            self.line[self.line_length.get()].set(chr);
            self.line_length.set(self.line_length.get() + 1);
            self.output.stdio().put_byte(chr);
            return false;
        }
        if chr == ESCAPE {
            self.line_length.set(0);
            return true;
        }
        if chr == BACK_SPACE || chr == BACK_SPACE_7F {
            if self.line_length.get() > 0 {
                self.output.write_raw(&[BACK_SPACE, b' ', BACK_SPACE]);
                self.line_length.set(self.line_length.get() - 1);
            }
            return false;
        }
        chr == CARRIAGE_RETURN || chr == LINE_FEED
    }

    /// How many bytes the line holds, for tests.
    #[cfg(test)]
    pub(crate) fn line_length(&self) -> usize {
        self.line_length.get()
    }

    fn print_newline(&self) {
        self.output.write_raw(b"\r\n");
    }

    fn print_prompt(&self) {
        self.print_newline();
        self.output.write_raw(b"> ");
    }
}

impl OnLineProcessed for StdioConsoleInput {
    fn on_line_processed(&self) {
        self.line_length.set(0);
        self.is_suspended.set(false);
        self.print_prompt();
    }
}
