// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! A stream into a byte buffer, ported from `util/stream/ByteBufferOutputStream`.

use super::OutputStream;

/// Writes bytes into a buffer. Writes beyond the end are dropped, but the position
/// advances as if the buffer were unlimited, so [`position`](Self::position) tells how
/// many bytes the complete input needs.
pub struct ByteBufferOutputStream<'b> {
    buffer: &'b mut [u8],
    /// Position of the next free byte.
    position: usize,
}

impl<'b> ByteBufferOutputStream<'b> {
    /// A stream into `buffer`.
    pub fn new(buffer: &'b mut [u8]) -> Self {
        Self { buffer, position: 0 }
    }

    /// Move the position ahead by `bytes_to_skip`.
    pub fn skip(&mut self, bytes_to_skip: usize) {
        self.position += bytes_to_skip;
    }

    /// Whether more was written than fits.
    pub fn is_overflow(&self) -> bool {
        self.position > self.buffer.len()
    }

    /// The number of bytes written, which may exceed the buffer's size.
    pub fn position(&self) -> usize {
        self.position
    }

    /// The bytes written, as many as fit.
    pub fn buffer(&self) -> &[u8] {
        &self.buffer[..self.position.min(self.buffer.len())]
    }
}

impl OutputStream for ByteBufferOutputStream<'_> {
    fn is_eof(&self) -> bool {
        self.position >= self.buffer.len()
    }

    fn write(&mut self, data: u8) {
        if self.position < self.buffer.len() {
            self.buffer[self.position] = data;
        }
        self.position += 1;
    }

    fn write_bytes(&mut self, buffer: &[u8]) {
        if self.position < self.buffer.len() {
            let bytes_to_copy = buffer.len().min(self.buffer.len() - self.position);
            self.buffer[self.position..self.position + bytes_to_copy]
                .copy_from_slice(&buffer[..bytes_to_copy]);
        }
        self.position += buffer.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ported from util/test/src/util/stream/ByteBufferOutputStreamTest.cpp.
    #[test]
    fn eof_for_an_empty_buffer() {
        let mut empty: [u8; 0] = [];
        let stream = ByteBufferOutputStream::new(&mut empty);
        assert!(stream.is_eof());
    }

    #[test]
    fn buffer_and_position() {
        let mut buffer = [0u8; 5];
        let mut stream = ByteBufferOutputStream::new(&mut buffer);
        assert!(stream.buffer().is_empty());
        assert_eq!(stream.position(), 0);
        for value in 1..=3u8 {
            stream.write(value);
            assert_eq!(stream.position(), usize::from(value));
        }
        assert_eq!(stream.buffer(), &[1, 2, 3]);
        for value in 4..=6u8 {
            stream.write(value);
            assert_eq!(stream.position(), usize::from(value));
        }
        assert_eq!(stream.buffer(), &[1, 2, 3, 4, 5]);
    }

    #[test]
    fn eof_while_space_is_left() {
        let mut buffer = [0u8; 10];
        let mut stream = ByteBufferOutputStream::new(&mut buffer);
        for _ in 0..10 {
            assert!(!stream.is_eof());
            stream.write(0xFF);
        }
        assert!(stream.is_eof());
    }

    #[test]
    fn write_fills_the_buffer() {
        let mut buffer = [0u8; 10];
        {
            let mut stream = ByteBufferOutputStream::new(&mut buffer);
            while !stream.is_eof() {
                stream.write(0x77);
            }
        }
        assert!(buffer.iter().all(|&b| b == 0x77));
    }

    #[test]
    fn overflow_after_eof() {
        let mut buffer = [0u8; 10];
        let mut stream = ByteBufferOutputStream::new(&mut buffer);
        while !stream.is_eof() {
            stream.write(0x77);
            assert!(!stream.is_overflow());
        }
        stream.write(0x77);
        assert!(stream.is_overflow());
    }

    #[test]
    fn write_bytes_cuts_at_the_end() {
        let mut array = [0xAFu8; 10];
        {
            let mut stream = ByteBufferOutputStream::new(&mut array[..6]);
            stream.write(b'T');
            stream.write_bytes(b"abc");
            stream.write_bytes(b"bcd");
            stream.write_bytes(b"abc");
            assert_eq!(stream.position(), 10);
            assert!(stream.is_eof());
        }
        assert_eq!(array, [b'T', b'a', b'b', b'c', b'b', b'c', 0xAF, 0xAF, 0xAF, 0xAF]);
    }

    #[test]
    fn skip() {
        let mut buffer = [3u8; 3];
        {
            let mut stream = ByteBufferOutputStream::new(&mut buffer);
            stream.skip(1);
            stream.write(1);
            assert!(!stream.is_eof());
            stream.write(2);
            assert!(stream.is_eof());
        }
        assert_eq!(buffer, [3, 1, 2]);
        let mut buffer = [3u8; 3];
        {
            let mut stream = ByteBufferOutputStream::new(&mut buffer);
            stream.write(1);
            stream.skip(10);
            assert!(stream.is_eof());
        }
        assert_eq!(buffer, [1, 3, 3]);
    }
}
