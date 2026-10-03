// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The chaining text writer, ported from `util/format/StringWriter` and
//! `SharedStringWriter`.

use super::formatter::PrintfFormatter;
use super::printf::{Arg, ArgumentReader, SliceArgumentReader};
use crate::stream::{OutputStream, SharedOutputStream};

/// Formats text to an output stream.
///
/// Every output method returns the writer, so calls chain within one statement. For the
/// supported format strings see [`PrintfFormatter`].
pub struct StringWriter<'s> {
    stream: &'s mut dyn OutputStream,
}

/// Something that writes to a [`StringWriter`] when applied: the port of the C++
/// function objects taken by `StringWriter::apply`.
pub trait Manipulator {
    /// Write to `writer`.
    fn apply(&self, writer: &mut StringWriter<'_>);
}

impl<'s> StringWriter<'s> {
    /// A writer into `stream`.
    pub fn new(stream: &'s mut dyn OutputStream) -> Self {
        Self { stream }
    }

    /// Write an end-of-line character (`\n`).
    pub fn endl(&mut self) -> &mut Self {
        self.stream.write(b'\n');
        self
    }

    /// Write a single character.
    pub fn write_char(&mut self, c: u8) -> &mut Self {
        self.stream.write(c);
        self
    }

    /// Write a string.
    pub fn write(&mut self, string: impl AsRef<[u8]>) -> &mut Self {
        self.stream.write_bytes(string.as_ref());
        self
    }

    /// Write a string that may be a null pointer, which writes nothing.
    pub fn write_opt(&mut self, string: Option<&[u8]>) -> &mut Self {
        if let Some(string) = string {
            self.stream.write_bytes(string);
        }
        self
    }

    /// Write `format` with `args`.
    pub fn printf(&mut self, format: &[u8], args: &[Arg<'_>]) -> &mut Self {
        self.vprintf(format, &mut SliceArgumentReader::new(args))
    }

    /// Write `format`, reading each argument from `reader`.
    pub fn vprintf(&mut self, format: &[u8], reader: &mut dyn ArgumentReader<'_>) -> &mut Self {
        PrintfFormatter::new(self.stream, true).format(format, reader);
        self
    }

    /// Apply a manipulator to this writer.
    pub fn apply(&mut self, manipulator: &dyn Manipulator) -> &mut Self {
        manipulator.apply(self);
        self
    }
}

/// Run `f` with a writer on `shared`'s output, started and ended around the call: the
/// port of `SharedStringWriter`, whose lifetime brackets the output.
pub fn with_shared_writer(
    shared: &mut dyn SharedOutputStream,
    f: impl FnOnce(&mut StringWriter<'_>),
) {
    let mut f = Some(f);
    shared.with_output(None, &mut |stream| {
        if let Some(f) = f.take() {
            f(&mut StringWriter::new(stream));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::{ContinuousUser, StringBufferOutputStream};

    struct Ext0;

    impl Manipulator for Ext0 {
        fn apply(&self, writer: &mut StringWriter<'_>) {
            writer.printf(b"ext0", &[]);
        }
    }

    // Ported from util/test/src/util/format/StringWriterTest.cpp (testMixedUsage).
    #[test]
    fn mixed_usage() {
        let mut buffer = [0u8; 40];
        let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
        {
            let mut cut = StringWriter::new(&mut stream);
            cut.write_char(b't')
                .write(b"abcdef1234:")
                .printf(b"%d", &[10_i32.into()])
                .printf(b"", &[])
                .write("test")
                .write_opt(None)
                .write(&b"test"[..3])
                .write("ABCD")
                .endl();
            cut.vprintf(b"%d", &mut SliceArgumentReader::new(&[10_i32.into()]));
        }
        assert_eq!(stream.string(), b"tabcdef1234:10testtesABCD\n10");
    }

    // testExtensions
    #[test]
    fn extensions() {
        let mut buffer = [0u8; 40];
        let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
        StringWriter::new(&mut stream).apply(&Ext0);
        assert_eq!(stream.string(), b"ext0");
    }

    struct Shared<'a> {
        started: bool,
        stream: &'a mut dyn OutputStream,
    }

    impl SharedOutputStream for Shared<'_> {
        fn with_output(
            &mut self,
            _user: Option<&'static dyn ContinuousUser>,
            f: &mut dyn FnMut(&mut dyn OutputStream),
        ) {
            self.started = true;
            f(self.stream);
            self.started = false;
        }

        fn release_continuous_user(&mut self, _user: &dyn ContinuousUser) {}
    }

    // Ported from SharedStringWriterTest.cpp: the output is started around the writer.
    #[test]
    fn shared_writer_brackets_the_output() {
        let mut buffer = [0u8; 40];
        let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
        let mut shared = Shared { started: false, stream: &mut stream };
        with_shared_writer(&mut shared, |writer| {
            writer.printf(b"Test output.", &[]);
        });
        assert!(!shared.started);
        assert_eq!(stream.string(), b"Test output.");
    }
}
