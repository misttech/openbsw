// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! A stream into a character buffer, ported from `util/stream/StringBufferOutputStream`.

use super::OutputStream;
use crate::string::c_str;

/// A lightweight output stream into a fixed buffer, like a bounded `stringstream`.
///
/// The buffer always ends in a NUL terminator once finalized ([`string`](Self::string) or
/// drop). When the text does not fit, it is cut so that the ellipsis and the end-of-string
/// suffix fit in.
pub struct StringBufferOutputStream<'b> {
    buffer: &'b mut [u8],
    end_of_string: &'b [u8],
    ellipsis: &'b [u8],
    current_index: usize,
    overflow: bool,
}

/// The length of `value`, at most `max_length`, stopping at a NUL.
fn bounded_string_length(value: &[u8], max_length: usize) -> usize {
    c_str(value).len().min(max_length)
}

impl<'b> StringBufferOutputStream<'b> {
    /// A stream into `buffer`, appending `end_of_string` to the text and `ellipsis` where
    /// text was cut off; both may be empty.
    pub fn new(buffer: &'b mut [u8], end_of_string: &'b [u8], ellipsis: &'b [u8]) -> Self {
        Self { buffer, end_of_string, ellipsis, current_index: 0, overflow: false }
    }

    /// Start over with an empty buffer.
    pub fn reset(&mut self) {
        self.current_index = 0;
        self.overflow = false;
    }

    /// The finalized text, without the NUL terminator.
    pub fn string(&mut self) -> &[u8] {
        self.finalize_buffer();
        c_str(self.buffer)
    }

    /// The finalized text including the NUL terminator.
    pub fn buffer(&mut self) -> &[u8] {
        let length = self.string().len();
        &self.buffer[..length + 1]
    }

    fn finalize_buffer(&mut self) {
        let buffer_size = self.buffer.len();
        if buffer_size == 0 {
            return;
        }
        let end_of_string_len = bounded_string_length(self.end_of_string, buffer_size - 1);
        let required_suffix = end_of_string_len + 1;
        let mut write_index = self.current_index;
        if self.overflow || write_index + required_suffix > buffer_size {
            let available_for_ellipsis = buffer_size - required_suffix;
            let ellipsis_len = bounded_string_length(self.ellipsis, available_for_ellipsis);
            write_index = available_for_ellipsis - ellipsis_len;
            if ellipsis_len > 0 {
                self.buffer[write_index..write_index + ellipsis_len]
                    .copy_from_slice(&self.ellipsis[..ellipsis_len]);
                write_index += ellipsis_len;
            }
            self.current_index = write_index;
        }
        if end_of_string_len > 0 {
            self.buffer[write_index..write_index + end_of_string_len]
                .copy_from_slice(&self.end_of_string[..end_of_string_len]);
        }
        self.buffer[write_index + end_of_string_len] = 0;
    }
}

impl OutputStream for StringBufferOutputStream<'_> {
    fn is_eof(&self) -> bool {
        self.current_index + 1 >= self.buffer.len()
    }

    fn write(&mut self, data: u8) {
        if self.current_index < self.buffer.len() {
            self.buffer[self.current_index] = data;
            self.current_index += 1;
        }
    }

    fn write_bytes(&mut self, buffer: &[u8]) {
        let mut size = buffer.len();
        if self.current_index + size > self.buffer.len() {
            size = self.buffer.len() - self.current_index;
            self.overflow = true;
        }
        self.buffer[self.current_index..self.current_index + size].copy_from_slice(&buffer[..size]);
        self.current_index += size;
    }
}

impl Drop for StringBufferOutputStream<'_> {
    fn drop(&mut self) {
        self.finalize_buffer();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ported from util/test/src/util/stream/StringBufferOutputStreamTest.cpp.
    #[test]
    fn append_is_safe() {
        let mut buffer = [0x17u8; 10];
        {
            let mut cut = StringBufferOutputStream::new(&mut buffer[..9], b"", b"");
            cut.write_bytes(b"abc");
            cut.write_bytes(b"def");
            cut.write_bytes(b"1234");
            assert_eq!(cut.string(), b"abcdef12");
        }
        assert_eq!(buffer[9], 0x17);
    }

    #[test]
    fn eof_with_write() {
        let mut buffer = [0x17u8; 10];
        let mut cut = StringBufferOutputStream::new(&mut buffer[..7], b"", b"");
        assert!(!cut.is_eof());
        cut.write(b'a');
        assert!(!cut.is_eof());
        cut.write_bytes(b"bcde");
        assert!(!cut.is_eof());
        cut.write(b'E');
        assert!(cut.is_eof());
        cut.write(b'E');
        assert!(cut.is_eof());
        cut.write(b'E');
        assert!(cut.is_eof());
        assert_eq!(cut.string(), b"abcdeE");
    }

    #[test]
    fn eof_with_write_buffer() {
        let mut buffer = [0x17u8; 10];
        let mut cut = StringBufferOutputStream::new(&mut buffer[..7], b"", b"");
        assert!(!cut.is_eof());
        cut.write(b'a');
        assert!(!cut.is_eof());
        cut.write_bytes(b"bcd");
        assert!(!cut.is_eof());
        cut.write_bytes(b"1234");
        assert!(cut.is_eof());
        assert_eq!(cut.string(), b"abcd12");
    }

    #[test]
    fn eol_is_appended() {
        let mut buffer = [0x17u8; 10];
        {
            let mut cut = StringBufferOutputStream::new(&mut buffer[..9], b"\n", b"");
            cut.write_bytes(b"abcdef1234");
            assert_eq!(cut.string(), b"abcdef1\n");
        }
        assert_eq!(buffer[9], 0x17);
    }

    #[test]
    fn eol_and_ellipsis_let_buffer_overflow() {
        let mut buffer = [0x17u8; 10];
        {
            let mut cut = StringBufferOutputStream::new(&mut buffer[..9], b"\n", b"");
            cut.write_bytes(b"abcdef12");
            assert_eq!(cut.string(), b"abcdef1\n");
        }
        assert_eq!(buffer[9], 0x17);
    }

    #[test]
    fn eol_and_ellipsis_are_appended() {
        let mut buffer = [0x17u8; 10];
        {
            let mut cut = StringBufferOutputStream::new(&mut buffer[..9], b"\n", b"..");
            cut.write_bytes(b"abcdef1234");
            assert_eq!(cut.string(), b"abcde..\n");
        }
        assert_eq!(buffer[9], 0x17);
    }

    #[test]
    fn reset() {
        let mut buffer = [0u8; 10];
        let mut cut = StringBufferOutputStream::new(&mut buffer, b"", b"");
        cut.write_bytes(b"abcdef1234");
        assert_eq!(cut.string(), b"abcdef123");
        cut.reset();
        cut.write_bytes(b"ABCDEFG");
        assert_eq!(cut.string(), b"ABCDEFG");
    }

    #[test]
    fn buffer_if_not_filled_completely() {
        let mut buffer = [0u8; 10];
        let mut cut = StringBufferOutputStream::new(&mut buffer, b"", b"");
        cut.write_bytes(b"abcd");
        assert_eq!(cut.buffer(), b"abcd\0");
    }

    #[test]
    fn buffer_if_full() {
        let mut buffer = [0u8; 10];
        let mut cut = StringBufferOutputStream::new(&mut buffer[..9], b"\n", b"..");
        cut.write_bytes(b"abcd1234");
        assert_eq!(cut.buffer().len(), 9);
    }

    #[test]
    fn string_with_empty_buffer_leaves_storage_untouched() {
        let mut buffer = [0x17u8; 1];
        {
            let mut cut = StringBufferOutputStream::new(&mut buffer[..0], b"\n", b"..");
            assert_eq!(cut.string(), b"");
        }
        assert_eq!(buffer[0], 0x17);
    }

    #[test]
    fn end_of_string_is_truncated_to_fit() {
        let mut buffer = [0x17u8; 5];
        {
            let mut cut = StringBufferOutputStream::new(&mut buffer[..4], b"ABCDE", b"");
            assert_eq!(cut.string(), b"ABC");
        }
        assert_eq!(buffer[4], 0x17);
    }

    #[test]
    fn ellipsis_is_truncated_to_fit() {
        let mut buffer = [0x17u8; 7];
        let mut cut = StringBufferOutputStream::new(&mut buffer, b"\n", b"......");
        cut.write_bytes(b"abcdefgh");
        assert_eq!(cut.string(), b".....\n");
    }

    #[test]
    fn ellipsis_is_omitted_if_no_space_is_available() {
        let mut buffer = [0x17u8; 2];
        let mut cut = StringBufferOutputStream::new(&mut buffer, b"\n", b"..");
        cut.write_bytes(b"ab");
        assert_eq!(cut.string(), b"\n");
    }

    #[test]
    fn suffix_can_trigger_truncation_without_write_overflow() {
        let mut buffer = [0x17u8; 6];
        let mut cut = StringBufferOutputStream::new(&mut buffer, b"\n", b"..");
        cut.write_bytes(b"abcde");
        assert_eq!(cut.string(), b"ab..\n");
    }

    #[test]
    fn drop_finalizes_buffer() {
        let mut buffer = [0x17u8; 10];
        {
            let mut cut = StringBufferOutputStream::new(&mut buffer[..9], b"\n", b"..");
            cut.write_bytes(b"abcdef1234");
        }
        assert_eq!(c_str(&buffer), b"abcde..\n");
        assert_eq!(buffer[9], 0x17);
    }

    #[test]
    fn mixed_usage() {
        let mut buffer = [0u8; 20];
        let mut cut = StringBufferOutputStream::new(&mut buffer, b"E", b"");
        cut.write_bytes(b"abcdef1234:");
        cut.write(b'1');
        cut.write(b'0');
        cut.write_bytes(b"test");
        cut.write_bytes(b"");
        assert_eq!(cut.string(), b"abcdef1234:10testE");
    }
}
