// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The printf format string scanner, ported from `util/format/PrintfFormatScanner`.

use super::printf::{ParamDatatype, ParamInfo, ParamType, ParamWidthOrPrecision, flags};

/// The kind of the current token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenType {
    /// One or more ordinary characters that are simply put out.
    String,
    /// A conversion (starting with `%`) of one or more arguments, described by
    /// [`PrintfFormatScanner::param_info`].
    Param,
    /// No more tokens; successive calls to `next_token` keep returning `End`.
    End,
}

/// Splits a format string into string and parameter tokens.
///
/// The scanner starts on the first token. The token's characters are available with
/// [`token`](Self::token) for both kinds; a parameter's details with
/// [`param_info`](Self::param_info).
pub struct PrintfFormatScanner<'a> {
    format: &'a [u8],
    start: usize,
    position: usize,
    token_type: TokenType,
    param_info: ParamInfo,
}

impl<'a> PrintfFormatScanner<'a> {
    /// Scan `format`, which may be empty, and position on the first token.
    pub fn new(format: &'a [u8]) -> Self {
        let mut scanner = Self {
            format,
            start: 0,
            position: 0,
            token_type: TokenType::End,
            param_info: ParamInfo::new(ParamType::Int, 0, ParamDatatype::Uint8),
        };
        scanner.next_token();
        scanner
    }

    /// Whether there is a token (the token type is not `End`).
    pub fn has_token(&self) -> bool {
        self.token_type != TokenType::End
    }

    /// Scan the next token.
    pub fn next_token(&mut self) {
        if self.current_char() == b'%' {
            self.scan_param();
        } else {
            self.scan_string(0);
        }
    }

    /// The type of the current token.
    pub fn token_type(&self) -> TokenType {
        self.token_type
    }

    /// The characters of the current token; empty for `End`.
    pub fn token(&self) -> &'a [u8] {
        &self.format[self.start..self.position]
    }

    /// The details of the current parameter conversion.
    ///
    /// Only meaningful while the token type is `Param`; before that the fields hold the
    /// values of the previous parameter, or defaults.
    pub fn param_info(&self) -> &ParamInfo {
        &self.param_info
    }

    /// Whether the current conversion takes its width from an argument (`*`).
    pub fn needs_width_param(&self) -> bool {
        self.param_info.width == ParamWidthOrPrecision::PARAM
    }

    /// Set the width of the current conversion.
    pub fn set_width(&mut self, width: i32) {
        self.param_info.width = width;
    }

    /// Whether the current conversion takes its precision from an argument (`.*`).
    pub fn needs_precision_param(&self) -> bool {
        self.param_info.precision == ParamWidthOrPrecision::PARAM
    }

    /// Set the precision of the current conversion.
    pub fn set_precision(&mut self, precision: i32) {
        self.param_info.precision = precision;
    }

    fn current_char(&self) -> u8 {
        self.format.get(self.position).copied().unwrap_or(0)
    }

    fn scan_string(&mut self, offset: usize) {
        self.start = self.position;
        if self.current_char() != 0 {
            self.token_type = TokenType::String;
            self.position += offset;
            while self.current_char() != 0 && self.current_char() != b'%' {
                self.position += 1;
            }
        } else {
            self.token_type = TokenType::End;
        }
    }

    fn scan_param(&mut self) {
        self.token_type = TokenType::Param;
        self.start = self.position;
        self.param_info.flags = 0;
        self.position += 1;
        self.scan_param_flags();
        self.param_info.width = self.scan_width_or_precision();
        if self.current_char() == b'.' {
            self.position += 1;
            self.param_info.precision = self.scan_width_or_precision();
        } else {
            self.param_info.precision = ParamWidthOrPrecision::DEFAULT;
        }
        let int_power = self.scan_param_length();
        self.scan_param_format_specifier(int_power);
    }

    fn scan_width_or_precision(&mut self) -> i32 {
        if self.current_char().is_ascii_digit() {
            let mut result: i32 = 0;
            while self.current_char().is_ascii_digit() {
                result =
                    result.wrapping_mul(10).wrapping_add(i32::from(self.current_char() - b'0'));
                self.position += 1;
            }
            result
        } else if self.current_char() == b'*' {
            self.position += 1;
            ParamWidthOrPrecision::PARAM
        } else {
            ParamWidthOrPrecision::DEFAULT
        }
    }

    fn scan_param_flags(&mut self) {
        loop {
            match self.current_char() {
                b'-' => self.param_info.flags |= flags::LEFT,
                b'+' => self.param_info.flags |= flags::PLUS,
                b' ' => self.param_info.flags |= flags::SPACE,
                b'#' => self.param_info.flags |= flags::ALT,
                b'0' => self.param_info.flags |= flags::ZEROPAD,
                _ => return,
            }
            self.position += 1;
        }
    }

    /// The length modifier as a power added to the byte datatype: `h` selects 16 bits,
    /// `l` and `ll` 64 bits, `L` and none 32 bits.
    fn scan_param_length(&mut self) -> u8 {
        match self.current_char() {
            b'h' => {
                self.position += 1;
                1
            }
            b'l' => {
                self.position += 1;
                if self.current_char() == b'l' {
                    self.position += 1;
                }
                3
            }
            b'L' => {
                self.position += 1;
                2
            }
            _ => 2,
        }
    }

    fn set_param_type(&mut self, kind: ParamType, datatype: ParamDatatype, base: u8) {
        self.param_info.kind = kind;
        self.param_info.base = base;
        self.param_info.datatype = datatype;
    }

    fn set_int_param_type(&mut self, power: u8, byte_datatype: ParamDatatype, base: u8) {
        self.set_param_type(ParamType::Int, byte_datatype.widened(power), base);
    }

    fn scan_param_format_specifier(&mut self, int_power: u8) {
        let specifier = self.current_char();
        self.position += 1;
        match specifier {
            b'c' => self.set_param_type(ParamType::Char, ParamDatatype::Uint8, 0),
            b'd' | b'i' => self.set_int_param_type(int_power, ParamDatatype::Sint8, 10),
            b'u' => self.set_int_param_type(int_power, ParamDatatype::Uint8, 10),
            b'o' => self.set_int_param_type(int_power, ParamDatatype::Uint8, 8),
            b'n' => self.set_param_type(ParamType::Pos, ParamDatatype::Sint32Ptr, 0),
            b'X' => {
                self.param_info.flags |= flags::UPPER;
                self.set_int_param_type(int_power, ParamDatatype::Uint8, 16);
            }
            b'x' => self.set_int_param_type(int_power, ParamDatatype::Uint8, 16),
            b'p' => self.set_param_type(ParamType::Ptr, ParamDatatype::VoidPtr, 16),
            b's' => self.set_param_type(ParamType::String, ParamDatatype::CharPtr, 0),
            b'S' => self.set_param_type(ParamType::String, ParamDatatype::SizedCharPtr, 0),
            // A `%` ending the string is dropped.
            0 => {
                self.position -= 1;
                self.scan_string(0);
            }
            // Any other character ends the conversion: the text from that character on
            // is an ordinary string token, so `%%` prints `%`.
            _ => {
                self.start += 1;
                self.position -= 1;
                self.scan_string(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_end_token(scanner: &PrintfFormatScanner<'_>) {
        assert!(!scanner.has_token());
        assert_eq!(scanner.token_type(), TokenType::End);
        assert!(scanner.token().is_empty());
    }

    fn check_string_token(scanner: &PrintfFormatScanner<'_>, expected: &[u8]) {
        assert!(scanner.has_token());
        assert_eq!(scanner.token_type(), TokenType::String);
        assert_eq!(scanner.token(), expected);
    }

    #[expect(clippy::too_many_arguments, reason = "mirrors the C++ test helper")]
    fn check_param_token(
        scanner: &PrintfFormatScanner<'_>,
        expected: &[u8],
        flags: u8,
        base: u8,
        width: i32,
        precision: i32,
        kind: ParamType,
        datatype: ParamDatatype,
    ) {
        assert!(scanner.has_token());
        assert_eq!(scanner.token_type(), TokenType::Param);
        assert_eq!(scanner.token(), expected);
        let info = scanner.param_info();
        assert_eq!(info.flags, flags);
        assert_eq!(info.base, base);
        assert_eq!(info.width, width);
        assert_eq!(info.precision, precision);
        assert_eq!(info.kind, kind);
        assert_eq!(info.datatype, datatype);
    }

    // Ported from util/test/src/util/format/PrintfFormatScannerTest.cpp (testFormats).
    #[test]
    fn scans_strings_and_parameters() {
        let mut scanner = PrintfFormatScanner::new(
            b"abcdefg%- hcbetween%+#012s%+#012S%33.ln%3.4QTest%0*.*d%0ld%0lld%0",
        );
        check_string_token(&scanner, b"abcdefg");
        scanner.next_token();
        check_param_token(
            &scanner,
            b"%- hc",
            flags::LEFT | flags::SPACE,
            0,
            ParamWidthOrPrecision::DEFAULT,
            ParamWidthOrPrecision::DEFAULT,
            ParamType::Char,
            ParamDatatype::Uint8,
        );
        scanner.next_token();
        check_string_token(&scanner, b"between");
        scanner.next_token();
        check_param_token(
            &scanner,
            b"%+#012s",
            flags::PLUS | flags::ALT | flags::ZEROPAD,
            0,
            12,
            ParamWidthOrPrecision::DEFAULT,
            ParamType::String,
            ParamDatatype::CharPtr,
        );
        scanner.next_token();
        check_param_token(
            &scanner,
            b"%+#012S",
            flags::PLUS | flags::ALT | flags::ZEROPAD,
            0,
            12,
            ParamWidthOrPrecision::DEFAULT,
            ParamType::String,
            ParamDatatype::SizedCharPtr,
        );
        scanner.next_token();
        check_param_token(
            &scanner,
            b"%33.ln",
            0,
            0,
            33,
            ParamWidthOrPrecision::DEFAULT,
            ParamType::Pos,
            ParamDatatype::Sint32Ptr,
        );
        scanner.next_token();
        check_string_token(&scanner, b"QTest");
        scanner.next_token();
        check_param_token(
            &scanner,
            b"%0*.*d",
            flags::ZEROPAD,
            10,
            ParamWidthOrPrecision::PARAM,
            ParamWidthOrPrecision::PARAM,
            ParamType::Int,
            ParamDatatype::Sint32,
        );
        scanner.next_token();
        check_param_token(
            &scanner,
            b"%0ld",
            flags::ZEROPAD,
            10,
            ParamWidthOrPrecision::DEFAULT,
            ParamWidthOrPrecision::DEFAULT,
            ParamType::Int,
            ParamDatatype::Sint64,
        );
        scanner.next_token();
        check_param_token(
            &scanner,
            b"%0lld",
            flags::ZEROPAD,
            10,
            ParamWidthOrPrecision::DEFAULT,
            ParamWidthOrPrecision::DEFAULT,
            ParamType::Int,
            ParamDatatype::Sint64,
        );
        scanner.next_token();
        check_end_token(&scanner);
        scanner.next_token();
        check_end_token(&scanner);
    }

    // testZeroFormat: a null format string scans as an end token.
    #[test]
    fn an_empty_format_is_an_end_token() {
        let scanner = PrintfFormatScanner::new(b"");
        check_end_token(&scanner);
    }

    #[test]
    fn width_and_precision_parameters_can_be_set() {
        let mut scanner = PrintfFormatScanner::new(b"%*.*d");
        assert!(scanner.needs_width_param());
        assert!(scanner.needs_precision_param());
        scanner.set_width(4);
        scanner.set_precision(2);
        assert!(!scanner.needs_width_param());
        assert!(!scanner.needs_precision_param());
        assert_eq!(scanner.param_info().width, 4);
        assert_eq!(scanner.param_info().precision, 2);
    }
}
