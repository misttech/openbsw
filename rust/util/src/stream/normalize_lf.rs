// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Line-feed normalization, ported from `util/stream/NormalizeLfOutputStream`.

use super::OutputStream;

/// Replaces every `\n` with a configurable sequence, `\r\n` by default.
pub struct NormalizeLfOutputStream<'s> {
    stream: &'s mut dyn OutputStream,
    crlf: &'s [u8],
}

impl<'s> NormalizeLfOutputStream<'s> {
    /// Write to `stream`, replacing `\n` with `crlf` (`\r\n` when `None`).
    pub fn new(stream: &'s mut dyn OutputStream, crlf: Option<&'s [u8]>) -> Self {
        Self { stream, crlf: crlf.unwrap_or(b"\r\n") }
    }
}

impl OutputStream for NormalizeLfOutputStream<'_> {
    fn is_eof(&self) -> bool {
        self.stream.is_eof()
    }

    fn write(&mut self, data: u8) {
        if data == b'\n' {
            for &c in self.crlf {
                self.stream.write(c);
            }
        } else {
            self.stream.write(data);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::StringBufferOutputStream;

    // Ported from util/test/src/util/stream/NormalizeLfOutputStreamTest.cpp.
    #[test]
    fn lf_is_replaced_with_custom_string() {
        let mut buffer = [0u8; 40];
        let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
        {
            let mut cut = NormalizeLfOutputStream::new(&mut stream, Some(b"[CRLF]"));
            cut.write(b'a');
            cut.write(b'\n');
            cut.write_bytes(b"abc\ndef");
            cut.write_bytes(b"AB\nDEF");
        }
        assert_eq!(stream.string(), b"a[CRLF]abc[CRLF]defAB[CRLF]DEF");
    }

    #[test]
    fn lf_is_replaced_with_default_string() {
        let mut buffer = [0u8; 40];
        let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
        {
            let mut cut = NormalizeLfOutputStream::new(&mut stream, None);
            cut.write(b'a');
            cut.write(b'\n');
            cut.write_bytes(b"abc\ndef");
            cut.write_bytes(b"AB\nDEF");
        }
        assert_eq!(stream.string(), b"a\r\nabc\r\ndefAB\r\nDEF");
    }

    #[test]
    fn eof_is_reported_correctly() {
        let mut buffer = [0u8; 10];
        let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
        let mut cut = NormalizeLfOutputStream::new(&mut stream, Some(b"[CRLF]"));
        assert!(!cut.is_eof());
        cut.write(b'a');
        cut.write(b'b');
        cut.write(b'\n');
        assert!(!cut.is_eof());
        cut.write(b'a');
        assert!(cut.is_eof());
    }
}
