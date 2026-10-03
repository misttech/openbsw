// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! A stream that drops everything, ported from `util/stream/NullOutputStream`.

use super::OutputStream;

/// A stream that is always at its end and ignores every write.
#[derive(Debug, Default)]
pub struct NullOutputStream;

impl OutputStream for NullOutputStream {
    fn is_eof(&self) -> bool {
        true
    }

    fn write(&mut self, _data: u8) {}

    fn write_bytes(&mut self, _buffer: &[u8]) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ported from util/test/src/util/stream/NullOutputStreamTest.cpp.
    #[test]
    fn nothing_is_done() {
        let mut cut = NullOutputStream;
        assert!(cut.is_eof());
        cut.write(b'a');
        cut.write_bytes(b"abc\ndef");
    }
}
