// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! VT100 escape sequences for attributed strings, ported from
//! `util/format/Vt100AttributedStringFormatter`.

use core::cell::Cell;

use super::attributed::{AttributedString, Color, NUMBER_OF_FORMATS, StringAttributes};
use super::string_writer::{Manipulator, StringWriter};

/// Keeps track of the VT100 text attributes in effect and writes only the codes needed
/// to change them.
///
/// Attributes are not set immediately: each call returns a [`Manipulator`] that a
/// [`StringWriter`] applies. The formatter must outlive the manipulators, since it keeps
/// the current attributes.
///
/// ```ignore
/// writer.printf(b"Mixed ", &[])
///     .apply(&vt100.attr(StringAttributes::new(Color::Green, BOLD, Color::DefaultColor)))
///     .printf(b"%s", &["string".into()])
///     .apply(&vt100.attr(StringAttributes::color(Color::Yellow)))
///     .printf(b" attributes", &[])
///     .apply(&vt100.reset());
/// ```
#[derive(Default)]
pub struct Vt100AttributedStringFormatter {
    attributes: Cell<StringAttributes>,
}

/// Applies attributes when written.
pub struct ApplyAttributes<'f> {
    formatter: &'f Vt100AttributedStringFormatter,
    attributes: StringAttributes,
}

/// Writes an attributed string, restoring the previous attributes afterwards.
pub struct WriteAttributedString<'f, 'a> {
    formatter: &'f Vt100AttributedStringFormatter,
    string: AttributedString<'a>,
}

const RESET_FORMAT_CODE: u8 = 0;
const FOREGROUND_COLOR_CODES: [u8; Color::NUMBER_OF_COLORS] =
    [39, 30, 31, 32, 33, 34, 35, 36, 37, 90, 91, 92, 93, 94, 95, 96, 97];
const BACKGROUND_COLOR_CODES: [u8; Color::NUMBER_OF_COLORS] =
    [49, 40, 41, 42, 43, 44, 45, 46, 47, 100, 101, 102, 103, 104, 105, 106, 107];
const FORMATTING_CODES: [u8; NUMBER_OF_FORMATS as usize] = [1, 2, 4, 5, 7, 8];

impl Vt100AttributedStringFormatter {
    /// A formatter with default attributes in effect.
    pub const fn new() -> Self {
        Self {
            attributes: Cell::new(StringAttributes::new(
                Color::DefaultColor,
                0,
                Color::DefaultColor,
            )),
        }
    }

    /// A manipulator that resets all attributes to default.
    pub fn reset(&self) -> ApplyAttributes<'_> {
        self.attr(StringAttributes::default())
    }

    /// A manipulator that sets the attributes.
    pub fn attr(&self, attributes: StringAttributes) -> ApplyAttributes<'_> {
        ApplyAttributes { formatter: self, attributes }
    }

    /// A manipulator that writes an attributed string and restores the previous
    /// attributes.
    pub fn write<'a>(&self, string: AttributedString<'a>) -> WriteAttributedString<'_, 'a> {
        WriteAttributedString { formatter: self, string }
    }

    fn write_string(&self, writer: &mut StringWriter<'_>, string: &AttributedString<'_>) {
        let previous = self.attributes.get();
        self.write_attributes(writer, *string.attributes());
        writer.write(string.string());
        self.write_attributes(writer, previous);
    }

    fn write_attributes(&self, writer: &mut StringWriter<'_>, attributes: StringAttributes) {
        if attributes != self.attributes.get() {
            if self.attributes.get().is_attributed() {
                Self::write_codes(writer, &[RESET_FORMAT_CODE]);
            }
            self.attributes.set(attributes);
            let mut codes = [0u8; 2 + NUMBER_OF_FORMATS as usize];
            let count = Self::format_codes(&attributes, &mut codes);
            Self::write_codes(writer, &codes[..count]);
        }
    }

    fn write_codes(writer: &mut StringWriter<'_>, codes: &[u8]) {
        if !codes.is_empty() {
            writer.write(b"\x1b[");
            for (i, code) in codes.iter().enumerate() {
                if i != 0 {
                    writer.write(b";");
                }
                writer.printf(b"%d", &[(*code).into()]);
            }
            writer.write(b"m");
        }
    }

    fn format_codes(attributes: &StringAttributes, codes: &mut [u8]) -> usize {
        let mut count = 0;
        if attributes.foreground_color() != Color::DefaultColor {
            codes[count] = FOREGROUND_COLOR_CODES[attributes.foreground_color() as usize];
            count += 1;
        }
        if attributes.background_color() != Color::DefaultColor {
            codes[count] = BACKGROUND_COLOR_CODES[attributes.background_color() as usize];
            count += 1;
        }
        if attributes.format() != 0 {
            for (check_format, code) in FORMATTING_CODES.iter().enumerate() {
                if (attributes.format() & (1 << check_format)) != 0 {
                    codes[count] = *code;
                    count += 1;
                }
            }
        }
        count
    }
}

impl Manipulator for ApplyAttributes<'_> {
    fn apply(&self, writer: &mut StringWriter<'_>) {
        self.formatter.write_attributes(writer, self.attributes);
    }
}

impl Manipulator for WriteAttributedString<'_, '_> {
    fn apply(&self, writer: &mut StringWriter<'_>) {
        self.formatter.write_string(writer, &self.string);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::BOLD;
    use crate::stream::StringBufferOutputStream;

    // Ported from util/test/src/util/format/Vt100AttributedStringFormatterTest.cpp.
    #[test]
    fn apply_zero_length_codes() {
        let mut buffer = [0u8; 40];
        let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
        let cut = Vt100AttributedStringFormatter::new();
        StringWriter::new(&mut stream).apply(&cut.attr(StringAttributes::default()));
        assert_eq!(stream.string(), b"");
    }

    #[test]
    fn apply_formats() {
        let mut buffer = [0u8; 40];
        let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
        let cut = Vt100AttributedStringFormatter::new();
        StringWriter::new(&mut stream)
            .apply(&cut.reset())
            .write(b"FF")
            .apply(&cut.attr(StringAttributes::new(Color::White, BOLD, Color::Red)))
            .write(b"abcd")
            .apply(&cut.attr(StringAttributes::new(Color::White, BOLD, Color::Red)))
            .write(b"ABCD")
            .apply(&cut.attr(StringAttributes::new(Color::DefaultColor, 0, Color::Black)))
            .write(b"EE")
            .apply(&cut.reset());
        assert_eq!(stream.string(), b"FF\x1b[97;41;1mabcdABCD\x1b[0m\x1b[40mEE\x1b[0m");
    }

    #[test]
    fn write_with_attributes() {
        let mut buffer = [0u8; 60];
        let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
        let cut = Vt100AttributedStringFormatter::new();
        StringWriter::new(&mut stream)
            .apply(&cut.reset())
            .write(b"FF")
            .apply(&cut.attr(StringAttributes::new(Color::White, BOLD, Color::Red)))
            .write(b"abcd")
            .apply(&cut.write(AttributedString::new(
                b"ABCD",
                StringAttributes::new(Color::DefaultColor, 0, Color::Black),
            )))
            .write(b"EE")
            .apply(&cut.reset());
        assert_eq!(
            stream.string(),
            b"FF\x1b[97;41;1mabcd\x1b[0m\x1b[40mABCD\x1b[0m\x1b[97;41;1mEE\x1b[0m"
        );
    }

    #[test]
    fn default_attributes_write_plain_text() {
        let mut buffer = [0u8; 60];
        let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
        let cut = Vt100AttributedStringFormatter::new();
        StringWriter::new(&mut stream)
            .apply(&cut.write(AttributedString::new(b"DEMO", StringAttributes::default())))
            .write(b": ")
            .apply(&cut.write(AttributedString::new(
                b"LIFECYCLE",
                StringAttributes::color(Color::DarkGray),
            )));
        assert_eq!(stream.string(), b"DEMO: \x1b[90mLIFECYCLE\x1b[0m");
    }
}
