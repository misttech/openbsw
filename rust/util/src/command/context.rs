// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The command-line tokenizer, ported from `util/command/CommandContext`.

use super::CommandResult;
use crate::stream::{ContinuousUser, NullOutputStream, OutputStream, SharedOutputStream};
use crate::string::compare_ignore_case;

/// Tokenizes a command's arguments, records the first error, and gives the command its
/// output stream.
pub struct CommandContext<'a, 'o> {
    shared: Option<&'o mut dyn SharedOutputStream>,
    null_stream: NullOutputStream,
    line: &'a [u8],
    position: usize,
    token_start: usize,
    result: CommandResult,
}

/// Matches an identifier token against candidates: the port of
/// `CommandContext::IdentifierChecker`.
pub struct IdentifierChecker<'c, 'a, 'o, T: Copy> {
    context: &'c mut CommandContext<'a, 'o>,
    identifier: &'a [u8],
    matched: Option<T>,
}

impl<'a, 'o> CommandContext<'a, 'o> {
    /// A context over `line`, writing to `shared` (or nowhere).
    pub fn new(line: &'a [u8], shared: Option<&'o mut dyn SharedOutputStream>) -> Self {
        let mut context = Self {
            shared,
            null_stream: NullOutputStream,
            line,
            position: 0,
            token_start: 0,
            result: CommandResult::Ok,
        };
        context.ignore_whitespace();
        context
    }

    /// Whether a token is left and no error occurred.
    pub fn has_token(&self) -> bool {
        self.is_valid() && self.position != self.line.len()
    }

    /// Scan a token up to the next whitespace. An empty token is a `BadToken`.
    pub fn scan_token(&mut self) -> &'a [u8] {
        let start = self.position;
        if self.is_valid() {
            self.token_start = self.position;
            while self.position != self.line.len() && !is_whitespace(self.current_char()) {
                self.position += 1;
            }
        }
        if self.check(self.position != start, CommandResult::BadToken) {
            let end = self.position;
            self.ignore_whitespace();
            return &self.line[start..end];
        }
        b""
    }

    /// Scan an identifier: letters, `_`, and digits after the first character.
    pub fn scan_identifier_token(&mut self) -> &'a [u8] {
        let start = self.position;
        if self.is_valid() {
            self.token_start = self.position;
            while self.position != self.line.len()
                && is_identifier_char(self.current_char(), self.position == self.token_start)
            {
                self.position += 1;
            }
        }
        let end = self.position;
        let scanned = self.position != start;
        let condition = scanned && self.ignore_whitespace();
        if self.check(condition, CommandResult::BadToken) {
            return &self.line[start..end];
        }
        b""
    }

    /// Scan pairs of hex digits into `buf`; returns the bytes scanned, or an empty slice
    /// on error.
    pub fn scan_byte_buffer_token<'b>(&mut self, buf: &'b mut [u8]) -> &'b mut [u8] {
        let start = self.position;
        let mut pos = 0;
        if self.is_valid() {
            self.token_start = self.position;
            while self.position != self.line.len() && self.position + 1 != self.line.len() {
                let hi_nibble = digit(self.current_char(), 16);
                if hi_nibble >= 0 && self.check(pos < buf.len(), CommandResult::BadValue) {
                    self.position += 1;
                    let lo_nibble = digit(self.current_char(), 16);
                    if self.check(lo_nibble >= 0, CommandResult::BadToken) {
                        buf[pos] = ((hi_nibble as u8) << 4) | lo_nibble as u8;
                        pos += 1;
                        self.position += 1;
                    }
                } else {
                    break;
                }
            }
        }
        let scanned = self.position != start;
        let condition = scanned && self.ignore_whitespace();
        if self.check(condition, CommandResult::BadToken) {
            return &mut buf[..pos];
        }
        &mut buf[..0]
    }

    /// Scan an identifier and match it against candidates.
    pub fn scan_enum_token<T: Copy>(&mut self) -> IdentifierChecker<'_, 'a, 'o, T> {
        let identifier = self.scan_identifier_token();
        IdentifierChecker { context: self, identifier, matched: None }
    }

    /// Scan an integer: decimal, `0x` hex, or octal with a leading `0`, with an optional
    /// sign. Returns zero on error.
    pub fn scan_int_token<T: IntToken>(&mut self) -> T {
        let mut result = T::ZERO;
        let mut negative = false;
        let mut start = self.position;
        let end = self.line.len();
        if self.is_valid() {
            self.token_start = self.position;
            let mut base: u32 = 10;
            match self.current_char() {
                b'+' | b'-' => {
                    negative = self.current_char() == b'-';
                    self.position += 1;
                }
                b'0' => {
                    if self.position + 1 < end
                        && (self.line[self.position + 1] == b'x'
                            || self.line[self.position + 1] == b'X')
                    {
                        base = 16;
                        self.position += 2;
                    } else {
                        base = 8;
                    }
                }
                _ => {}
            }
            start = self.position;
            while self.position != end {
                let digit = digit(self.current_char(), base);
                if digit < 0 {
                    break;
                }
                result = result.mul_add(base, digit as u32);
                self.position += 1;
            }
        }
        let mut condition = self.position != start;
        if condition {
            condition = self.ignore_whitespace();
        }
        if self.check(condition, CommandResult::BadToken) {
            return if negative { result.negated() } else { result };
        }
        T::ZERO
    }

    /// Check that the line has ended; otherwise the result is `UnexpectedToken`.
    pub fn check_eol(&mut self) -> bool {
        self.check(self.position == self.line.len(), CommandResult::UnexpectedToken)
    }

    /// Record `result` if `condition` is false and no error was recorded yet. Returns
    /// whether the context was valid before the check.
    pub fn check(&mut self, condition: bool, result: CommandResult) -> bool {
        if self.is_valid() && !condition {
            self.result = result;
        }
        self.is_valid()
    }

    /// The result so far.
    pub fn result(&self) -> CommandResult {
        self.result
    }

    /// The rest of the line: from the offending token for `BadValue`, else from the
    /// current position.
    pub fn suffix(&self) -> &'a [u8] {
        let start =
            if self.result == CommandResult::BadValue { self.token_start } else { self.position };
        &self.line[start..]
    }

    fn is_valid(&self) -> bool {
        self.result == CommandResult::Ok
    }

    fn current_char(&self) -> u8 {
        self.line.get(self.position).copied().unwrap_or(0)
    }

    /// Skip whitespace; true if some was skipped or the line ended.
    fn ignore_whitespace(&mut self) -> bool {
        let start = self.position;
        while self.position != self.line.len() && is_whitespace(self.current_char()) {
            self.position += 1;
        }
        self.position != start || self.position == self.line.len()
    }
}

impl SharedOutputStream for CommandContext<'_, '_> {
    fn with_output(
        &mut self,
        user: Option<&'static dyn ContinuousUser>,
        f: &mut dyn FnMut(&mut dyn OutputStream),
    ) {
        match self.shared.as_mut() {
            Some(shared) => shared.with_output(None, &mut |stream| {
                f(stream);
                if let Some(user) = user {
                    user.end_continuous_output(stream);
                }
            }),
            None => f(&mut self.null_stream),
        }
    }

    fn release_continuous_user(&mut self, _user: &dyn ContinuousUser) {}
}

impl<T: Copy> IdentifierChecker<'_, '_, '_, T> {
    /// Accept `value` if the identifier equals `identifier`, ignoring case.
    pub fn check(mut self, identifier: &[u8], value: T) -> Self {
        if self.matched.is_none() && compare_ignore_case(self.identifier, identifier) == 0 {
            self.matched = Some(value);
        }
        self
    }

    /// The matched value, or `None` after recording `BadValue` in the context.
    pub fn value(self) -> Option<T> {
        self.context.check(self.matched.is_some(), CommandResult::BadValue);
        self.matched
    }
}

/// The integer types [`CommandContext::scan_int_token`] can scan. Arithmetic wraps, as
/// the C++ template's does on unsigned types.
pub trait IntToken: Copy {
    /// Zero.
    const ZERO: Self;
    /// `self * base + digit`, wrapping.
    fn mul_add(self, base: u32, digit: u32) -> Self;
    /// `-self`, wrapping.
    fn negated(self) -> Self;
}

macro_rules! int_token {
    ($($t:ty),*) => { $(
        impl IntToken for $t {
            const ZERO: Self = 0;
            fn mul_add(self, base: u32, digit: u32) -> Self {
                self.wrapping_mul(base as $t).wrapping_add(digit as $t)
            }
            fn negated(self) -> Self {
                self.wrapping_neg()
            }
        }
    )* };
}
int_token!(u8, u16, u32, u64, i8, i16, i32, i64);

fn is_whitespace(c: u8) -> bool {
    matches!(c, b'\r' | b'\n' | b'\t' | b' ')
}

fn is_identifier_char(c: u8, first_char: bool) -> bool {
    if c.is_ascii_alphabetic() || c == b'_' {
        return true;
    }
    if !first_char {
        return digit(c, 10) >= 0;
    }
    false
}

fn digit(c: u8, base: u32) -> i32 {
    if (b'0'..=b'7').contains(&c) {
        return i32::from(c - b'0');
    }
    if base < 10 {
        return -1;
    }
    if c == b'8' || c == b'9' {
        return i32::from(c - b'0');
    }
    if base != 16 {
        return -1;
    }
    if c.is_ascii_lowercase() && c <= b'f' {
        return i32::from(c - b'a') + 10;
    }
    if c.is_ascii_uppercase() && c <= b'F' {
        return i32::from(c - b'A') + 10;
    }
    -1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::with_shared_writer;
    use crate::stream::{SharedOutputStreamImpl, StringBufferOutputStream};
    use core::sync::atomic::{AtomicU32, Ordering};

    // Ported from util/test/src/util/command/CommandContextTest.cpp (testScanTokens).
    #[test]
    fn scan_tokens() {
        {
            let mut cut = CommandContext::new(b" \r\n\t ", None);
            assert!(!cut.has_token());
            assert!(cut.check_eol());
            assert_eq!(cut.result(), CommandResult::Ok);
        }
        {
            let mut cut = CommandContext::new(b"  /test  ", None);
            assert!(cut.has_token());
            assert_eq!(cut.scan_token(), b"/test");
            assert!(!cut.has_token());
            assert!(cut.check_eol());
            assert_eq!(cut.result(), CommandResult::Ok);
        }
        {
            let mut cut = CommandContext::new(b"  /test", None);
            assert!(cut.has_token());
            assert_eq!(cut.scan_token(), b"/test");
            assert!(!cut.has_token());
            assert_eq!(cut.scan_token(), b"");
            assert!(!cut.check_eol());
            assert_eq!(cut.scan_token(), b"");
            assert_eq!(cut.suffix(), b"");
            assert_eq!(cut.result(), CommandResult::BadToken);
        }
        {
            let mut cut = CommandContext::new(b"  /test 734U", None);
            assert!(cut.has_token());
            assert_eq!(cut.scan_token(), b"/test");
            assert!(cut.has_token());
            assert_eq!(cut.scan_token(), b"734U");
            assert!(!cut.has_token());
            assert!(cut.check_eol());
            assert_eq!(cut.result(), CommandResult::Ok);
        }
        {
            let mut cut = CommandContext::new(b"  abc_093  ", None);
            assert!(cut.has_token());
            assert_eq!(cut.scan_identifier_token(), b"abc_093");
            assert!(!cut.has_token());
            assert!(cut.check_eol());
            assert_eq!(cut.result(), CommandResult::Ok);
        }
        {
            let mut cut = CommandContext::new(b"  abc{  ", None);
            assert!(cut.has_token());
            assert_eq!(cut.scan_identifier_token(), b"");
            assert!(!cut.has_token());
            assert_eq!(cut.suffix(), b"{  ");
            assert!(!cut.check_eol());
            assert_eq!(cut.result(), CommandResult::BadToken);
        }
        {
            let mut cut = CommandContext::new(b"  1ab  ", None);
            assert!(cut.has_token());
            assert_eq!(cut.scan_identifier_token(), b"");
            assert!(!cut.has_token());
            assert_eq!(cut.suffix(), b"1ab  ");
            assert!(!cut.check_eol());
            assert_eq!(cut.result(), CommandResult::BadToken);
        }
        {
            let mut cut = CommandContext::new(b" test unknown", None);
            assert!(cut.has_token());
            assert_eq!(
                cut.scan_enum_token::<u32>()
                    .check(b"abc", 1)
                    .check(b"test", 12)
                    .check(b"test", 13)
                    .value(),
                Some(12)
            );
            assert_eq!(cut.result(), CommandResult::Ok);
            assert!(cut.has_token());
            assert_eq!(cut.suffix(), b"unknown");
            assert_eq!(
                cut.scan_enum_token::<u32>()
                    .check(b"abc", 1)
                    .check(b"test", 12)
                    .check(b"test", 13)
                    .value(),
                None
            );
            assert!(!cut.check_eol());
            assert_eq!(cut.suffix(), b"unknown");
            assert_eq!(cut.result(), CommandResult::BadValue);
        }
        {
            let mut cut = CommandContext::new(b"  0", None);
            assert!(cut.has_token());
            assert_eq!(cut.scan_int_token::<u32>(), 0);
            assert!(!cut.has_token());
            assert!(cut.check_eol());
            assert_eq!(cut.result(), CommandResult::Ok);
        }
        {
            let mut cut = CommandContext::new(b"  +12234  ", None);
            assert!(cut.has_token());
            assert_eq!(cut.scan_int_token::<u32>(), 12234);
            assert!(!cut.has_token());
            assert!(cut.check_eol());
            assert_eq!(cut.result(), CommandResult::Ok);
        }
        {
            let mut cut = CommandContext::new(b"  -3473  0x3438 0Xabc 0xF01 023 0x34t", None);
            assert!(cut.has_token());
            assert_eq!(cut.scan_int_token::<i32>(), -3473);
            assert!(cut.has_token());
            assert_eq!(cut.suffix(), b"0x3438 0Xabc 0xF01 023 0x34t");
            assert_eq!(cut.scan_int_token::<i32>(), 0x3438);
            assert!(cut.has_token());
            assert_eq!(cut.suffix(), b"0Xabc 0xF01 023 0x34t");
            assert_eq!(cut.scan_int_token::<i32>(), 0xabc);
            assert!(cut.has_token());
            assert_eq!(cut.suffix(), b"0xF01 023 0x34t");
            assert_eq!(cut.scan_int_token::<i32>(), 0xf01);
            assert!(cut.has_token());
            assert_eq!(cut.suffix(), b"023 0x34t");
            assert_eq!(cut.scan_int_token::<i32>(), 19);
            assert!(cut.has_token());
            assert_eq!(cut.suffix(), b"0x34t");
            assert_eq!(cut.scan_int_token::<i32>(), 0);
            assert_eq!(cut.suffix(), b"t");
            assert!(!cut.check_eol());
            assert_eq!(cut.result(), CommandResult::BadToken);
            assert_eq!(cut.scan_int_token::<i32>(), 0);
            assert_eq!(cut.scan_identifier_token(), b"");
        }
        {
            let mut cut = CommandContext::new(b"  0238", None);
            assert!(cut.has_token());
            assert_eq!(cut.scan_int_token::<i32>(), 0);
            assert_eq!(cut.suffix(), b"8");
            assert!(!cut.check_eol());
            assert_eq!(cut.result(), CommandResult::BadToken);
        }
        {
            let mut cut = CommandContext::new(b" 0afe234ae1", None);
            let mut buffer = [0u8; 5];
            assert!(cut.has_token());
            assert_eq!(cut.scan_byte_buffer_token(&mut buffer), &[0x0a, 0xfe, 0x23, 0x4a, 0xe1]);
            assert!(!cut.has_token());
            assert!(cut.check_eol());
            assert_eq!(cut.result(), CommandResult::Ok);
        }
        {
            let mut cut = CommandContext::new(&b"0afe234ae1b8f"[..12], None);
            let mut buffer = [0u8; 6];
            assert!(cut.has_token());
            assert_eq!(
                cut.scan_byte_buffer_token(&mut buffer),
                &[0x0a, 0xfe, 0x23, 0x4a, 0xe1, 0xb8]
            );
            assert!(!cut.has_token());
            assert!(cut.check_eol());
            assert_eq!(cut.result(), CommandResult::Ok);
        }
        {
            let mut cut = CommandContext::new(b" 0afe234a  ", None);
            let mut buffer = [0u8; 5];
            assert!(cut.has_token());
            assert_eq!(cut.scan_byte_buffer_token(&mut buffer), &[0x0a, 0xfe, 0x23, 0x4a]);
            assert!(!cut.has_token());
            assert!(cut.check_eol());
            assert_eq!(cut.result(), CommandResult::Ok);
        }
        {
            let mut cut = CommandContext::new(b" 0afe234ae1ff", None);
            let mut buffer = [0u8; 5];
            assert!(cut.has_token());
            assert!(cut.scan_byte_buffer_token(&mut buffer).is_empty());
            assert!(!cut.has_token());
            assert!(!cut.check_eol());
            assert_eq!(cut.result(), CommandResult::BadValue);
        }
        {
            let mut cut = CommandContext::new(b" 0afe234 ", None);
            let mut buffer = [0u8; 5];
            assert!(cut.has_token());
            assert!(cut.scan_byte_buffer_token(&mut buffer).is_empty());
            assert!(!cut.has_token());
            assert!(!cut.check_eol());
            assert_eq!(cut.result(), CommandResult::BadToken);
        }
        {
            let mut cut = CommandContext::new(b" 0afe234 ", None);
            let mut buffer = [0u8; 5];
            assert!(cut.has_token());
            cut.check(false, CommandResult::Error);
            assert!(cut.scan_byte_buffer_token(&mut buffer).is_empty());
            assert!(!cut.has_token());
            assert!(!cut.check_eol());
            assert_eq!(cut.result(), CommandResult::Error);
        }
    }

    struct CountingUser(AtomicU32);

    impl ContinuousUser for CountingUser {
        fn end_continuous_output(&self, _stream: &mut dyn OutputStream) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    static USER: CountingUser = CountingUser(AtomicU32::new(0));

    // testStringWriter
    #[test]
    fn string_writer() {
        {
            let mut cut = CommandContext::new(b"", None);
            with_shared_writer(&mut cut, |writer| {
                writer.write_char(b'a');
                writer.printf(b"0", &[]);
            });
        }
        {
            let mut buffer = [0u8; 20];
            let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
            {
                let mut shared = SharedOutputStreamImpl::new(&mut stream);
                let mut cut = CommandContext::new(b"", Some(&mut shared));
                with_shared_writer(&mut cut, |writer| {
                    writer.write_char(b'a');
                    writer.printf(b"abc%d", &[1_i32.into()]);
                });
            }
            assert_eq!(stream.string(), b"aabc1");
        }
        {
            let mut buffer = [0u8; 20];
            let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
            let mut shared = SharedOutputStreamImpl::new(&mut stream);
            let mut cut = CommandContext::new(b"", Some(&mut shared));
            cut.with_output(Some(&USER), &mut |_| {});
            assert_eq!(USER.0.load(Ordering::Relaxed), 1);
            cut.release_continuous_user(&USER);
            cut.with_output(None, &mut |_| {});
            assert_eq!(USER.0.load(Ordering::Relaxed), 1);
        }
    }
}
