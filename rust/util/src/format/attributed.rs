// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Strings with display attributes, ported from `util/format/AttributedString`.

/// Colors that can be applied to attributed strings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Color {
    /// The terminal's default color.
    #[default]
    DefaultColor = 0,
    /// Black.
    Black,
    /// Red.
    Red,
    /// Green.
    Green,
    /// Yellow.
    Yellow,
    /// Blue.
    Blue,
    /// Magenta.
    Magenta,
    /// Cyan.
    Cyan,
    /// Light gray.
    LightGray,
    /// Dark gray.
    DarkGray,
    /// Light red.
    LightRed,
    /// Light green.
    LightGreen,
    /// Light yellow.
    LightYellow,
    /// Light blue.
    LightBlue,
    /// Light magenta.
    LightMagenta,
    /// Light cyan.
    LightCyan,
    /// White.
    White,
}

impl Color {
    /// The number of colors.
    pub const NUMBER_OF_COLORS: usize = 17;
}

/// Bold text.
pub const BOLD: u8 = 0x01;
/// Dim text.
pub const DIM: u8 = 0x02;
/// Underlined text.
pub const UNDERLINE: u8 = 0x04;
/// Blinking text.
pub const BLINK: u8 = 0x08;
/// Reversed foreground and background.
pub const REVERSE: u8 = 0x10;
/// Hidden text.
pub const HIDDEN: u8 = 0x20;
/// The number of formatting flags.
pub const NUMBER_OF_FORMATS: u8 = 6;

/// The attributes of a string: colors and a bit mask of formatting flags.
///
/// The port merges the C++ `PlainStringAttributes` (constant data) and
/// `StringAttributes` (its wrapper): a `const fn` constructor serves both uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StringAttributes {
    foreground_color: Color,
    format: u8,
    background_color: Color,
}

impl StringAttributes {
    /// Attributes with the given foreground color, formatting flags and background.
    pub const fn new(foreground_color: Color, format: u8, background_color: Color) -> Self {
        Self { foreground_color, format, background_color }
    }

    /// Attributes with only a foreground color.
    pub const fn color(foreground_color: Color) -> Self {
        Self::new(foreground_color, 0, Color::DefaultColor)
    }

    /// Whether any field has a non-default value.
    pub fn is_attributed(&self) -> bool {
        self.foreground_color != Color::DefaultColor
            || self.background_color != Color::DefaultColor
            || self.format != 0
    }

    /// The foreground color.
    pub fn foreground_color(&self) -> Color {
        self.foreground_color
    }

    /// The formatting flags.
    pub fn format(&self) -> u8 {
        self.format
    }

    /// The background color.
    pub fn background_color(&self) -> Color {
        self.background_color
    }
}

/// A string together with the attributes to apply to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttributedString<'a> {
    string: &'a [u8],
    attributes: StringAttributes,
}

impl<'a> AttributedString<'a> {
    /// `string` with `attributes`.
    pub const fn new(string: &'a [u8], attributes: StringAttributes) -> Self {
        Self { string, attributes }
    }

    /// The string.
    pub fn string(&self) -> &'a [u8] {
        self.string
    }

    /// The attributes to apply.
    pub fn attributes(&self) -> &StringAttributes {
        &self.attributes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ported from util/test/src/util/format/AttributedStringTest.cpp.
    #[test]
    fn constructors() {
        let attributes = StringAttributes::new(Color::Black, BLINK | REVERSE, Color::Red);
        let cut = AttributedString::new(b"text", attributes);
        assert_eq!(cut.string(), b"text");
        assert_eq!(
            *cut.attributes(),
            StringAttributes::new(Color::Black, BLINK | REVERSE, Color::Red)
        );
    }

    #[test]
    fn getters() {
        let cut = StringAttributes::new(Color::Black, BLINK | REVERSE, Color::Red);
        assert_eq!(cut.foreground_color(), Color::Black);
        assert_eq!(cut.format(), BLINK | REVERSE);
        assert_eq!(cut.background_color(), Color::Red);
    }

    #[test]
    fn is_attributed() {
        assert!(
            !StringAttributes::new(Color::DefaultColor, 0, Color::DefaultColor).is_attributed()
        );
        assert!(StringAttributes::new(Color::Black, 0, Color::DefaultColor).is_attributed());
        assert!(
            StringAttributes::new(Color::DefaultColor, BLINK | REVERSE, Color::DefaultColor)
                .is_attributed()
        );
        assert!(StringAttributes::new(Color::DefaultColor, 0, Color::Red).is_attributed());
    }

    #[test]
    fn comparison() {
        assert_eq!(StringAttributes::default(), StringAttributes::default());
        let base = StringAttributes::new(Color::Black, BLINK | REVERSE, Color::Red);
        assert_eq!(base, StringAttributes::new(Color::Black, BLINK | REVERSE, Color::Red));
        assert_ne!(base, StringAttributes::new(Color::Green, BLINK | REVERSE, Color::Red));
        assert_ne!(base, StringAttributes::new(Color::Black, BOLD, Color::Red));
        assert_ne!(base, StringAttributes::new(Color::Black, BLINK | REVERSE, Color::Yellow));
    }
}
