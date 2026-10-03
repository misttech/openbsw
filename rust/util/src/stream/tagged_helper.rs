// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Prefix and suffix per line, ported from `util/stream/TaggedOutputHelper`.

use super::OutputStream;

/// Writes a prefix at the start of every line and a suffix in place of every `\n`.
pub struct TaggedOutputHelper<'t> {
    prefix: &'t [u8],
    suffix: &'t [u8],
    line_start: bool,
}

impl<'t> TaggedOutputHelper<'t> {
    /// A helper with `prefix` and `suffix`, either of which may be empty.
    pub const fn new(prefix: &'t [u8], suffix: &'t [u8]) -> Self {
        Self { prefix, suffix, line_start: true }
    }

    /// Forget the current line: the next byte starts a new one.
    pub fn reset(&mut self) {
        self.line_start = true;
    }

    /// Whether the next byte starts a line.
    pub fn is_line_start(&self) -> bool {
        self.line_start
    }

    /// Write `buffer` to `stream`, tagging lines.
    pub fn write_bytes(&mut self, stream: &mut dyn OutputStream, buffer: &[u8]) {
        for &data in buffer {
            self.write_byte(stream, data);
        }
    }

    /// End the current line with the suffix, if one was started.
    pub fn end_line(&mut self, stream: &mut dyn OutputStream) {
        if !self.line_start {
            stream.write_bytes(self.suffix);
            self.line_start = true;
        }
    }

    fn write_byte(&mut self, stream: &mut dyn OutputStream, data: u8) {
        if self.line_start {
            self.line_start = false;
            stream.write_bytes(self.prefix);
        }
        if data == b'\n' {
            stream.write_bytes(self.suffix);
            self.line_start = true;
        } else {
            stream.write(data);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::StringBufferOutputStream;

    // Ported from util/test/src/util/stream/TaggedOutputHelperTest.cpp.
    #[test]
    fn prefix_and_suffixes() {
        for (prefix, suffix, expected) in
            [(&b""[..], &b""[..], &b"ab"[..]), (b"P", b"", b"PaPb"), (b"", b"S", b"aSb")]
        {
            let mut buffer = [0u8; 80];
            let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
            let mut cut = TaggedOutputHelper::new(prefix, suffix);
            cut.write_bytes(&mut stream, b"a\nb");
            assert_eq!(stream.string(), expected);
        }
    }

    #[test]
    fn reset() {
        let mut buffer = [0u8; 80];
        let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
        let mut cut = TaggedOutputHelper::new(b"P", b"S");
        cut.write_bytes(&mut stream, b"a\nb");
        cut.reset();
        cut.write_bytes(&mut stream, b"c");
        assert_eq!(stream.string(), b"PaSPbPc");
    }
}
