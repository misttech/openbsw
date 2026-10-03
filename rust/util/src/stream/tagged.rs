// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! A tagged stream, ported from `util/stream/TaggedOutputStream`.

use super::{OutputStream, TaggedOutputHelper};

/// An output stream that tags every line with a prefix and suffix. Dropping it ends an
/// open line.
pub struct TaggedOutputStream<'s> {
    stream: &'s mut dyn OutputStream,
    helper: TaggedOutputHelper<'s>,
}

impl<'s> TaggedOutputStream<'s> {
    /// Tag lines written to `stream` with `prefix` and `suffix`.
    pub fn new(stream: &'s mut dyn OutputStream, prefix: &'s [u8], suffix: &'s [u8]) -> Self {
        Self { stream, helper: TaggedOutputHelper::new(prefix, suffix) }
    }
}

impl OutputStream for TaggedOutputStream<'_> {
    fn is_eof(&self) -> bool {
        self.stream.is_eof()
    }

    fn write(&mut self, data: u8) {
        self.helper.write_bytes(self.stream, &[data]);
    }

    fn write_bytes(&mut self, buffer: &[u8]) {
        self.helper.write_bytes(self.stream, buffer);
    }
}

impl Drop for TaggedOutputStream<'_> {
    fn drop(&mut self) {
        self.helper.end_line(self.stream);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::StringBufferOutputStream;

    // Ported from util/test/src/util/stream/TaggedOutputStreamTest.cpp.
    #[test]
    fn prefix_and_suffixes_are_inserted() {
        let mut buffer = [0u8; 80];
        let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
        {
            let mut cut = TaggedOutputStream::new(&mut stream, b"[START]", b"[CRLF]");
            cut.write(b'a');
            cut.write(b'\n');
            cut.write_bytes(b"abc\ndef");
            cut.write_bytes(b"AB\nDEF");
        }
        assert_eq!(
            stream.string(),
            b"[START]a[CRLF][START]abc[CRLF][START]defAB[CRLF][START]DEF[CRLF]"
        );
        let mut buffer = [0u8; 80];
        let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
        {
            let mut cut = TaggedOutputStream::new(&mut stream, b"[START]", b"[CRLF]");
            cut.write(b'\n');
        }
        assert_eq!(stream.string(), b"[START][CRLF]");
    }

    #[test]
    fn eof_is_reported_correctly() {
        let mut buffer = [0u8; 10];
        let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
        let mut cut = TaggedOutputStream::new(&mut stream, b"", b"[CRLF]");
        assert!(!cut.is_eof());
        cut.write(b'a');
        cut.write(b'b');
        cut.write(b'\n');
        assert!(!cut.is_eof());
        cut.write(b'a');
        assert!(cut.is_eof());
    }
}
