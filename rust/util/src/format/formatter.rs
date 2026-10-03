// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The printf engine, ported from `util/format/PrintfFormatter`.

use super::printf::{
    Arg, ArgumentReader, ParamDatatype, ParamInfo, ParamType, ParamVariant, ParamWidthOrPrecision,
    SliceArgumentReader, flags,
};
use super::scanner::{PrintfFormatScanner, TokenType};
use crate::stream::OutputStream;
use crate::string::c_str;

/// Formats printf-style text into an output stream.
///
/// Supported conversions: `%c`, `%s` (a NUL-terminated string), `%S` (a sized string),
/// `%d`/`%i`, `%u`, `%o`, `%x`/`%X`, `%p`, `%n` and `%%`; the flags `-`, `+`, space, `#`
/// and `0`; a width and a precision, each a number or `*`; and the length modifiers `h`,
/// `l`, `ll` and `L`. A missing argument prints `<?>`, a null string `<NULL>`.
pub struct PrintfFormatter<'s> {
    stream: &'s mut dyn OutputStream,
    write_param: bool,
    pos: usize,
}

const INT_BUFFER_SIZE: usize = 22;

impl<'s> PrintfFormatter<'s> {
    /// A formatter writing to `stream`. With `write_param`, `%n` writes the number of
    /// characters written so far to its argument; without it, `%n` is skipped.
    pub fn new(stream: &'s mut dyn OutputStream, write_param: bool) -> Self {
        Self { stream, write_param, pos: 0 }
    }

    /// Format `format` with `args`, the counterpart of the variadic `format`.
    pub fn format_args(&mut self, format: &[u8], args: &[Arg<'_>]) {
        self.format(format, &mut SliceArgumentReader::new(args));
    }

    /// Format `format`, reading each argument from `reader`.
    pub fn format(&mut self, format: &[u8], reader: &mut dyn ArgumentReader<'_>) {
        let mut scanner = PrintfFormatScanner::new(format);
        while scanner.has_token() {
            if scanner.token_type() == TokenType::String {
                self.format_text(scanner.token());
            } else {
                if scanner.needs_width_param() {
                    let width = match reader.read_argument(ParamDatatype::Sint32) {
                        Some(ParamVariant::Int(value)) => value as u32 as i32,
                        _ => ParamWidthOrPrecision::DEFAULT,
                    };
                    scanner.set_width(width);
                }
                if scanner.needs_precision_param() {
                    let precision = match reader.read_argument(ParamDatatype::Sint32) {
                        Some(ParamVariant::Int(value)) => value as u32 as i32,
                        _ => ParamWidthOrPrecision::DEFAULT,
                    };
                    scanner.set_precision(precision);
                }
                match reader.read_argument(scanner.param_info().datatype) {
                    Some(argument) => self.format_param(scanner.param_info(), argument),
                    None => self.format_text(b"<?>"),
                }
            }
            scanner.next_token();
        }
    }

    /// Write `text` without scanning for format specifiers.
    pub fn format_text(&mut self, text: &[u8]) {
        self.put_string(text);
    }

    /// Convert one argument and write it.
    pub fn format_param(&mut self, info: &ParamInfo, variant: ParamVariant<'_>) {
        match info.kind {
            ParamType::Char => {
                let value = [variant_bits(variant) as u8];
                self.format_string_param(info, &value);
            }
            ParamType::String => match variant {
                ParamVariant::Str(Some(string)) if info.datatype == ParamDatatype::CharPtr => {
                    self.format_string_param(info, c_str(string));
                }
                ParamVariant::Str(Some(string)) => self.format_string_param(info, string),
                _ => self.format_string_param(info, b"<NULL>"),
            },
            ParamType::Int | ParamType::Ptr => self.format_int_param(info, variant),
            ParamType::Pos => {
                if let (true, ParamVariant::Pos(Some(position))) = (self.write_param, variant) {
                    position.set(self.pos as i32);
                }
            }
        }
    }

    fn format_string_param(&mut self, info: &ParamInfo, string: &[u8]) {
        let mut length = string.len();
        if info.precision >= 0 && length > info.precision as usize {
            length = info.precision as usize;
        }
        self.fill_width(info, length as i32, true);
        self.put_string(&string[..length]);
        self.fill_width(info, length as i32, false);
    }

    fn format_int_param(&mut self, info: &ParamInfo, variant: ParamVariant<'_>) {
        let mut buffer = [0u8; INT_BUFFER_SIZE];
        let (buffer_start, sign) = format_int_datatype(&mut buffer, info, variant_bits(variant));
        let sign_text = int_sign(info, sign);
        let prefix = int_prefix(info, sign);
        let mut digit_count = (INT_BUFFER_SIZE - buffer_start) as isize;
        let mut total_count = digit_count + sign_text.len() as isize + prefix.len() as isize;
        let precision: isize = if info.precision >= 0 {
            if info.precision == 0 && sign == 0 {
                digit_count = 0;
                if total_count > 0 {
                    total_count -= 1;
                }
            }
            if info.precision as isize > digit_count {
                info.precision as isize - digit_count
            } else {
                0
            }
        } else if (info.flags & flags::ZEROPAD) != 0 && (info.flags & flags::LEFT) == 0 {
            if info.width as isize > total_count { info.width as isize - total_count } else { 0 }
        } else {
            0
        };
        total_count += precision;
        self.fill_width(info, total_count as i32, true);
        self.put_string(sign_text);
        self.put_string(prefix);
        self.put_char(b'0', precision as usize);
        self.put_string(&buffer[buffer_start..buffer_start + digit_count as usize]);
        self.fill_width(info, total_count as i32, false);
    }

    fn fill_width(&mut self, info: &ParamInfo, length: i32, left: bool) {
        if info.width >= 0 && (((info.flags & flags::LEFT) != 0) != left) && info.width > length {
            self.put_char(b' ', (info.width - length) as usize);
        }
    }

    fn put_string(&mut self, string: &[u8]) {
        self.pos += string.len();
        self.stream.write_bytes(string);
    }

    fn put_char(&mut self, c: u8, count: usize) {
        self.pos += count;
        for _ in 0..count {
            self.stream.write(c);
        }
    }
}

/// The integer bits of a variant; strings and positions have none.
fn variant_bits(variant: ParamVariant<'_>) -> u64 {
    match variant {
        ParamVariant::Int(bits) => bits,
        ParamVariant::Str(_) | ParamVariant::Pos(_) => 0,
    }
}

/// Write the digits of `value` right-aligned into `buffer`; returns the index of the first
/// digit and the sign (-1, 0 for zero, +1).
fn format_int_digits<T: IntDigits>(
    buffer: &mut [u8],
    digits: &[u8],
    mut value: T,
    signed_type: bool,
    base: T,
) -> (usize, i8) {
    let mut index = buffer.len();
    let sign;
    if !value.is_zero() {
        if signed_type && value.is_negative() {
            value = value.wrapping_neg();
            sign = -1;
        } else {
            sign = 1;
        }
        while !value.is_zero() {
            index -= 1;
            buffer[index] = digits[value.rem(base)];
            value = value.div(base);
        }
    } else {
        sign = 0;
        index -= 1;
        buffer[index] = b'0';
    }
    (index, sign)
}

/// The unsigned integer operations `format_int_digits` needs, for `u32` and `u64`.
trait IntDigits: Copy {
    fn is_zero(self) -> bool;
    fn is_negative(self) -> bool;
    fn wrapping_neg(self) -> Self;
    fn rem(self, base: Self) -> usize;
    fn div(self, base: Self) -> Self;
}

impl IntDigits for u32 {
    fn is_zero(self) -> bool {
        self == 0
    }

    fn is_negative(self) -> bool {
        (self as i32) < 0
    }

    fn wrapping_neg(self) -> Self {
        u32::wrapping_neg(self)
    }

    fn rem(self, base: Self) -> usize {
        (self % base) as usize
    }

    fn div(self, base: Self) -> Self {
        self / base
    }
}

impl IntDigits for u64 {
    fn is_zero(self) -> bool {
        self == 0
    }

    fn is_negative(self) -> bool {
        (self as i64) < 0
    }

    fn wrapping_neg(self) -> Self {
        u64::wrapping_neg(self)
    }

    fn rem(self, base: Self) -> usize {
        (self % base) as usize
    }

    fn div(self, base: Self) -> Self {
        self / base
    }
}

/// Write the digits for `info.datatype`; returns the start index and the sign. An unknown
/// datatype writes nothing.
fn format_int_datatype(
    buffer: &mut [u8; INT_BUFFER_SIZE],
    info: &ParamInfo,
    bits: u64,
) -> (usize, i8) {
    let digits: &[u8] =
        if (info.flags & flags::UPPER) != 0 { b"0123456789ABCDEF" } else { b"0123456789abcdef" };
    let base32 = u32::from(info.base);
    let base64 = u64::from(info.base);
    match info.datatype {
        // A 16-bit value is sign-extended to 32 bits first, as the C++ cast does.
        ParamDatatype::Sint16 => {
            format_int_digits(buffer, digits, bits as u16 as i16 as i32 as u32, true, base32)
        }
        ParamDatatype::Sint32 => format_int_digits(buffer, digits, bits as u32, true, base32),
        ParamDatatype::Sint64 => format_int_digits(buffer, digits, bits, true, base64),
        ParamDatatype::Uint16 => {
            format_int_digits(buffer, digits, u32::from(bits as u16), false, base32)
        }
        ParamDatatype::Uint32 => format_int_digits(buffer, digits, bits as u32, false, base32),
        ParamDatatype::Uint64 => format_int_digits(buffer, digits, bits, false, base64),
        ParamDatatype::VoidPtr => {
            if core::mem::size_of::<usize>() == 4 {
                format_int_digits(buffer, digits, bits as u32, false, base32)
            } else {
                format_int_digits(buffer, digits, bits, false, base64)
            }
        }
        _ => (INT_BUFFER_SIZE, 0),
    }
}

fn int_sign(info: &ParamInfo, sign: i8) -> &'static [u8] {
    if info.base == 10 {
        match info.datatype {
            ParamDatatype::Sint8
            | ParamDatatype::Sint16
            | ParamDatatype::Sint32
            | ParamDatatype::Sint64 => {
                if sign < 0 {
                    return b"-";
                } else if (info.flags & flags::PLUS) != 0 {
                    return b"+";
                } else if (info.flags & flags::SPACE) != 0 {
                    return b" ";
                }
            }
            _ => {}
        }
    }
    b""
}

fn int_prefix(info: &ParamInfo, sign: i8) -> &'static [u8] {
    match info.base {
        8 if sign > 0 && (info.flags & flags::ALT) != 0 => b"0",
        16 if (info.flags & flags::ALT) != 0 => {
            if (info.flags & flags::UPPER) != 0 {
                b"0X"
            } else {
                b"0x"
            }
        }
        _ => b"",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::StringBufferOutputStream;
    use core::cell::Cell;
    use std::string::String;

    fn format(format: &[u8], args: &[Arg<'_>]) -> String {
        let mut buffer = [0u8; 300];
        let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
        PrintfFormatter::new(&mut stream, true).format_args(format, args);
        String::from_utf8(stream.string().to_vec()).unwrap()
    }

    fn expect(expected: &str, fmt: &[u8], args: &[Arg<'_>]) {
        assert_eq!(expected, format(fmt, args), "format {:?}", core::str::from_utf8(fmt).unwrap());
    }

    /// `expectAndCheckIntPrintf`: positive, negative and zero values of one format.
    fn expect_int<T>(fmt: &[u8], value: T, positive: &str, negative: Option<&str>, zero: &str)
    where
        T: Copy + core::ops::Neg<Output = T> + Default + Into<Arg<'static>>,
    {
        if let Some(negative) = negative {
            expect(negative, fmt, &[(-value).into()]);
        }
        expect(zero, fmt, &[T::default().into()]);
        expect(positive, fmt, &[value.into()]);
    }

    fn expect_uint<T: Copy + Default + Into<Arg<'static>>>(
        fmt: &[u8],
        value: T,
        positive: &str,
        zero: &str,
    ) {
        expect(zero, fmt, &[T::default().into()]);
        expect(positive, fmt, &[value.into()]);
    }

    // Ported from util/test/src/util/format/PrintfFormatterTest.cpp (testDefaultFormats).
    #[test]
    fn default_formats() {
        expect("  t", b"%03c", &['t'.into()]);
        expect("  t", b"%3c", &['t'.into()]);
        expect("         abc", b"%012s", &["abc".into()]);
        expect("abcdefghijklmnop", b"%12s", &["abcdefghijklmnop".into()]);
        expect("abcdefghijkl", b"%.12s", &["abcdefghijklmnop".into()]);
        expect("abcdefghi", b"%.12s", &["abcdefghi".into()]);
        expect("         abc", b"%#12s", &["abc".into()]);
        expect_int::<i32>(b"%d", 47, "47", Some("-47"), "0");
        expect_int::<i32>(b"%i", 47, "47", Some("-47"), "0");
        expect_int::<i32>(b"%Ld", 47, "47", None, "0");
        expect_int::<i64>(
            b"%ld",
            4234892348329347,
            "4234892348329347",
            Some("-4234892348329347"),
            "0",
        );
        expect_int::<i64>(
            b"%lld",
            4234892348329347,
            "4234892348329347",
            Some("-4234892348329347"),
            "0",
        );
        expect_uint::<u32>(b"% u", 47, "47", "0");
        expect_uint::<u32>(b"%+u", 47, "47", "0");
        expect_int::<i32>(b"% d", 47, " 47", Some("-47"), " 0");
        expect_int::<i32>(b"% +d", 47, "+47", Some("-47"), "+0");
        expect_int::<i32>(b"%+12d", 47, "         +47", Some("         -47"), "          +0");
        expect_int::<i32>(b"%+012d", 47, "+00000000047", Some("-00000000047"), "+00000000000");
        expect_int::<i32>(b"%-012d", 47, "47          ", Some("-47         "), "0           ");
        expect_int::<i32>(b"%+012.4d", 47, "       +0047", Some("       -0047"), "       +0000");
        expect_int::<i32>(b"%-012.4d", 47, "0047        ", Some("-0047       "), "0000        ");
        expect_uint::<u32>(b"%-012.4u", 47, "0047        ", "0000        ");
        expect_int::<i32>(b"%01d", 47, "47", Some("-47"), "0");
        expect_int::<i32>(b"%+12.4d", 47, "       +0047", Some("       -0047"), "       +0000");
        expect_int::<i16>(b"%+12hd", 47, "         +47", Some("         -47"), "          +0");
        expect_int::<i64>(
            b"%+16ld",
            24329482243247,
            " +24329482243247",
            Some(" -24329482243247"),
            "              +0",
        );
        expect_int::<i64>(
            b"%+16lld",
            24329482243247,
            " +24329482243247",
            Some(" -24329482243247"),
            "              +0",
        );
        expect_uint::<i64>(b"%+16llu", 24329482243247, "  24329482243247", "               0");
        expect_int::<i32>(b"%+0.4d", 47, "+0047", Some("-0047"), "+0000");
        expect_int::<i32>(b"%+.4d", 47, "+0047", Some("-0047"), "+0000");
        expect_int::<i32>(b"%+8.0d", 47, "     +47", Some("     -47"), "       +");
        expect_uint::<u32>(b"%x", 0x1a3c5, "1a3c5", "0");
        expect_uint::<u16>(b"%hx", 0x23c5, "23c5", "0");
        expect_uint::<u64>(b"%lx", 0x1a3c5d7e9f0, "1a3c5d7e9f0", "0");
        expect_uint::<u64>(b"%llx", 0x1a3c5d7e9f0, "1a3c5d7e9f0", "0");
        expect("  0x1a3c5", b"%#+9x", &[0x1a3c5_u32.into()]);
        expect("0X001A3C5", b"%#+09X", &[0x1a3c5_u32.into()]);
        expect_uint::<u32>(b"%o", 0o47, "47", "0");
        expect_uint::<u16>(b"%ho", 0o47, "47", "0");
        expect_uint::<u64>(b"%lo", 0o47234672362234, "47234672362234", "0");
        expect_uint::<u64>(b"%llo", 0o47234672362234, "47234672362234", "0");
        expect_uint::<u32>(b"%#+9o", 0o47, "      047", "        0");
        expect_uint::<u32>(b"%#+09o", 0o47, "000000047", "000000000");
        expect(" 0x1a3c5", b"%#*x", &[8_i32.into(), 0x1a3c5_u32.into()]);
        expect("0047", b"%#.*d", &[4_i32.into(), 47_i32.into()]);
        expect("0x1a3c5", b"%#p", &[0x1a3c5_usize.into()]);
        expect("Test%", b"Test%+# 9.0ll%", &[12_i32.into()]);
        expect("Test%Test", b"Test%%Test", &[12_i32.into()]);
    }

    // testExtendedFormats: %S with sized strings.
    #[test]
    fn extended_formats() {
        expect("         abc", b"%012S", &[b"abc".as_slice().into()]);
        expect("abcdefghijklmnop", b"%12S", &[b"abcdefghijklmnop".as_slice().into()]);
        expect("abcdefghijkl", b"%.12S", &[b"abcdefghijklmnop".as_slice().into()]);
        expect("abcdefghi", b"%.12S", &[b"abcdefghi".as_slice().into()]);
        expect("         abc", b"%#12S", &[b"abc".as_slice().into()]);
    }

    // testPositionParam
    #[test]
    fn position_param() {
        let pos1 = Cell::new(0);
        let pos2 = Cell::new(0);
        assert_eq!("abcdefgh", format(b"abcd%nefgh%n", &[(&pos1).into(), (&pos2).into()]));
        assert_eq!(pos1.get(), 4);
        assert_eq!(pos2.get(), 8);
        // A missing position argument is reported like any missing argument; the C++ test
        // passes a null pointer here, which the formatter skips.
        assert_eq!("abcd<?>efgh", format(b"abcd%nefgh", &[]));
    }

    // testNullPtr
    #[test]
    fn null_pointers() {
        assert_eq!("<NULL>", format(b"%s", &[Arg::Str(None)]));
        assert_eq!("<NULL>", format(b"%S", &[Arg::Str(None)]));
        assert_eq!("<?>", format(b"%*.*d", &[]));
    }

    // testFormatWithEllipsis
    #[test]
    fn format_with_arguments() {
        assert_eq!("17 abc", format(b"%d %s", &[17_i32.into(), "abc".into()]));
    }

    // testFormatParamWithInvalidValues: unknown datatypes print nothing, %n writes back
    // only with write_param.
    #[test]
    fn format_param_with_invalid_values() {
        let mut buffer = [0u8; 300];
        let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
        {
            let mut formatter = PrintfFormatter::new(&mut stream, true);
            formatter.format_param(
                &ParamInfo::new(ParamType::Int, 10, ParamDatatype::Count),
                ParamVariant::Int(0),
            );
            formatter.format_text(b".");
            formatter.format_param(
                &ParamInfo::new(ParamType::Int, 10, ParamDatatype::Sint8),
                ParamVariant::Int(16),
            );
            formatter.format_text(b".");
        }
        assert_eq!(stream.string(), b"..");
        let pos = Cell::new(17);
        {
            let mut formatter = PrintfFormatter::new(&mut stream, false);
            formatter.format_param(
                &ParamInfo::new(ParamType::Pos, 0, ParamDatatype::Sint32Ptr),
                ParamVariant::Pos(Some(&pos)),
            );
        }
        assert_eq!(pos.get(), 17);
    }

    #[test]
    fn negative_sixteen_bit_values_are_sign_extended() {
        assert_eq!("-1", format(b"%hd", &[(-1_i16).into()]));
        assert_eq!("65535", format(b"%hu", &[65535_u16.into()]));
        assert_eq!("-2147483648", format(b"%d", &[i32::MIN.into()]));
    }
}
