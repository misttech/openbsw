// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The console streams, ported from `util/stream/StdoutStream` and `BspStubs.h`.

use super::OutputStream;

/// The board's console bytes: the port of the `getByteFromStdin` and `putByteToStdout`
/// functions the BSP provides.
pub trait Stdio {
    /// The next byte from the console, or a negative value, or 255, when none is
    /// waiting. (The Zephyr BSP returns 255: it stores `-1` in an `unsigned char`.)
    fn get_byte(&self) -> i32;

    /// Write one byte to the console.
    fn put_byte(&self, byte: u8);
}

/// A stream writing to the console.
pub struct StdoutStream<'io> {
    stdio: &'io dyn Stdio,
}

impl<'io> StdoutStream<'io> {
    /// A stream writing through `stdio`.
    pub const fn new(stdio: &'io dyn Stdio) -> Self {
        Self { stdio }
    }
}

impl OutputStream for StdoutStream<'_> {
    fn is_eof(&self) -> bool {
        false
    }

    fn write(&mut self, data: u8) {
        self.stdio.put_byte(data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::RefCell;
    use std::vec::Vec;

    struct Recorder(RefCell<Vec<u8>>);

    impl Stdio for Recorder {
        fn get_byte(&self) -> i32 {
            -1
        }

        fn put_byte(&self, byte: u8) {
            self.0.borrow_mut().push(byte);
        }
    }

    // Ported from util/test/src/util/stream/StdoutStreamTest.cpp.
    #[test]
    fn writes_go_to_stdout() {
        let recorder = Recorder(RefCell::new(Vec::new()));
        let mut cut = StdoutStream::new(&recorder);
        assert!(!cut.is_eof());
        cut.write(b'a');
        cut.write_bytes(b"bcd");
        assert_eq!(recorder.0.borrow().as_slice(), b"abcd");
    }
}
