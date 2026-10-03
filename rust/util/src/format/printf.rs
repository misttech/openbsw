// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The printf parameter model, ported from `util/format/Printf.h`,
//! `IPrintfArgumentReader.h` and `PrintfArgumentReader.h`.
//!
//! C++ reads the arguments from a `va_list` as the datatype the format string asks for.
//! Rust has no variadic functions, so callers pass a slice of [`Arg`] values and the
//! [`SliceArgumentReader`] hands them out one by one, converted to the requested
//! [`ParamDatatype`]. An argument whose kind does not fit the conversion (a string for
//! `%d`) reads as missing, which the formatter prints as `<?>`; the C++ code would have
//! read garbage from the stack.

use core::cell::Cell;

/// The kind of conversion a format specifier selects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamType {
    /// Character conversion `%c`.
    Char,
    /// String conversion `%s`, `%S`.
    String,
    /// Pointer conversion `%p`.
    Ptr,
    /// Position pointer `%n`.
    Pos,
    /// Integer value conversions `%d`, `%i`, `%o`, `%x`, `%X`.
    Int,
}

/// Formatting hints, combined into [`ParamInfo::flags`].
pub mod flags {
    /// Values are aligned left within the width.
    pub const LEFT: u8 = 0x01;
    /// Signed non-negative values are prefixed with `+`.
    pub const PLUS: u8 = 0x02;
    /// Signed non-negative values are prefixed with a space.
    pub const SPACE: u8 = 0x04;
    /// The alternate output format for octal and hex values (prefix `0`, `0x` or `0X`).
    pub const ALT: u8 = 0x08;
    /// Numeric values are filled with `0` up to the width.
    pub const ZEROPAD: u8 = 0x10;
    /// Upper-case hex digits and prefix.
    pub const UPPER: u8 = 0x20;
}

/// The datatype a conversion reads its argument as.
///
/// The discriminants matter: the scanner adds a length power (0 for `h`, 1 default,
/// 2 for `l`) to `Uint8` or `Sint8` to select the width, as the C++ code does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ParamDatatype {
    /// `uint8_t`
    Uint8 = 0,
    /// `uint16_t`
    Uint16 = 1,
    /// `uint32_t`
    Uint32 = 2,
    /// `uint64_t`
    Uint64 = 3,
    /// `int8_t`
    Sint8 = 4,
    /// `int16_t`
    Sint16 = 5,
    /// `int32_t`
    Sint32 = 6,
    /// `int64_t`
    Sint64 = 7,
    /// `void const*`
    VoidPtr = 8,
    /// `char const*`
    CharPtr = 9,
    /// `PlainSizedString const*`
    SizedCharPtr = 10,
    /// `int32_t*`
    Sint32Ptr = 11,
    /// The number of datatypes; not a datatype.
    Count = 12,
}

impl ParamDatatype {
    /// The datatype with discriminant `value`, or `Count` for an unknown one.
    pub const fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Uint8,
            1 => Self::Uint16,
            2 => Self::Uint32,
            3 => Self::Uint64,
            4 => Self::Sint8,
            5 => Self::Sint16,
            6 => Self::Sint32,
            7 => Self::Sint64,
            8 => Self::VoidPtr,
            9 => Self::CharPtr,
            10 => Self::SizedCharPtr,
            11 => Self::Sint32Ptr,
            _ => Self::Count,
        }
    }

    /// The datatype `power` steps wider than this one, as the length modifiers select.
    pub const fn widened(self, power: u8) -> Self {
        Self::from_u8((self as u8).wrapping_add(power))
    }
}

/// Special values of [`ParamInfo::width`] and [`ParamInfo::precision`].
pub struct ParamWidthOrPrecision;

impl ParamWidthOrPrecision {
    /// Use the default (depending on the conversion type).
    pub const DEFAULT: i32 = -1;
    /// The value comes from an additional argument (`*`).
    pub const PARAM: i32 = -2;
}

/// Everything about a single parameter conversion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParamInfo {
    /// The conversion type.
    pub kind: ParamType,
    /// A bit mask of [`flags`].
    pub flags: u8,
    /// The base (8, 10 or 16) for integer conversions.
    pub base: u8,
    /// The datatype the argument is read as.
    pub datatype: ParamDatatype,
    /// The width, or one of [`ParamWidthOrPrecision`].
    pub width: i32,
    /// The precision, or one of [`ParamWidthOrPrecision`].
    pub precision: i32,
}

impl ParamInfo {
    /// A conversion with no flags, default width and precision.
    pub const fn new(kind: ParamType, base: u8, datatype: ParamDatatype) -> Self {
        Self {
            kind,
            flags: 0,
            base,
            datatype,
            width: ParamWidthOrPrecision::DEFAULT,
            precision: ParamWidthOrPrecision::DEFAULT,
        }
    }
}

/// An argument value as the formatter consumes it: the Rust rendering of the C++
/// `ParamVariant` union, which is read as the datatype the conversion asked for.
#[derive(Clone, Copy, Debug)]
pub enum ParamVariant<'a> {
    /// The bits of an integer or pointer. Signed sources are sign-extended, so reading
    /// the low 16 or 32 bits as signed gives the original value.
    Int(u64),
    /// A string for `%s` or `%S`; `None` is a null pointer.
    Str(Option<&'a [u8]>),
    /// The target of `%n`; `None` is a null pointer.
    Pos(Option<&'a Cell<i32>>),
}

/// An argument a caller passes to a formatting call.
#[derive(Clone, Copy, Debug)]
pub enum Arg<'a> {
    /// A signed integer of any width.
    Int(i64),
    /// An unsigned integer of any width, or a pointer value.
    Uint(u64),
    /// A string for `%s` or `%S`; `None` is a null pointer, printed as `<NULL>`.
    Str(Option<&'a [u8]>),
    /// The target of `%n`.
    Pos(&'a Cell<i32>),
}

macro_rules! arg_from_signed {
    ($($t:ty),*) => { $(impl From<$t> for Arg<'_> { fn from(value: $t) -> Self { Arg::Int(i64::from(value)) } })* };
}
macro_rules! arg_from_unsigned {
    ($($t:ty),*) => { $(impl From<$t> for Arg<'_> { fn from(value: $t) -> Self { Arg::Uint(u64::from(value)) } })* };
}
arg_from_signed!(i8, i16, i32, i64);
arg_from_unsigned!(u8, u16, u32, u64);

impl From<usize> for Arg<'_> {
    fn from(value: usize) -> Self {
        Arg::Uint(value as u64)
    }
}

impl From<char> for Arg<'_> {
    fn from(value: char) -> Self {
        Arg::Uint(u64::from(u32::from(value)))
    }
}

impl<'a> From<&'a str> for Arg<'a> {
    fn from(value: &'a str) -> Self {
        Arg::Str(Some(value.as_bytes()))
    }
}

impl<'a> From<&'a [u8]> for Arg<'a> {
    fn from(value: &'a [u8]) -> Self {
        Arg::Str(Some(value))
    }
}

impl<'a, const N: usize> From<&'a [u8; N]> for Arg<'a> {
    fn from(value: &'a [u8; N]) -> Self {
        Arg::Str(Some(value))
    }
}

impl<'a> From<&'a Cell<i32>> for Arg<'a> {
    fn from(value: &'a Cell<i32>) -> Self {
        Arg::Pos(value)
    }
}

/// Provides the data for a [`super::PrintfFormatter`]: the port of
/// `IPrintfArgumentReader`.
pub trait ArgumentReader<'a> {
    /// Get the next argument value, converted to the expected datatype.
    ///
    /// Returns `None` when no argument is left or the argument cannot be read as
    /// `datatype`.
    fn read_argument(&mut self, datatype: ParamDatatype) -> Option<ParamVariant<'a>>;
}

/// Reads arguments from a slice, in order: the port of `PrintfArgumentReader`, which
/// reads them from a `va_list`.
pub struct SliceArgumentReader<'a> {
    args: &'a [Arg<'a>],
    index: usize,
}

impl<'a> SliceArgumentReader<'a> {
    /// A reader over `args`.
    pub const fn new(args: &'a [Arg<'a>]) -> Self {
        Self { args, index: 0 }
    }
}

impl<'a> ArgumentReader<'a> for SliceArgumentReader<'a> {
    fn read_argument(&mut self, datatype: ParamDatatype) -> Option<ParamVariant<'a>> {
        let arg = *self.args.get(self.index)?;
        self.index += 1;
        match (datatype, arg) {
            (
                ParamDatatype::Uint8
                | ParamDatatype::Uint16
                | ParamDatatype::Uint32
                | ParamDatatype::Uint64
                | ParamDatatype::Sint8
                | ParamDatatype::Sint16
                | ParamDatatype::Sint32
                | ParamDatatype::Sint64
                | ParamDatatype::VoidPtr,
                Arg::Int(value),
            ) => Some(ParamVariant::Int(value as u64)),
            (
                ParamDatatype::Uint8
                | ParamDatatype::Uint16
                | ParamDatatype::Uint32
                | ParamDatatype::Uint64
                | ParamDatatype::Sint8
                | ParamDatatype::Sint16
                | ParamDatatype::Sint32
                | ParamDatatype::Sint64
                | ParamDatatype::VoidPtr,
                Arg::Uint(value),
            ) => Some(ParamVariant::Int(value)),
            (ParamDatatype::CharPtr | ParamDatatype::SizedCharPtr, Arg::Str(value)) => {
                Some(ParamVariant::Str(value))
            }
            (ParamDatatype::Sint32Ptr, Arg::Pos(value)) => Some(ParamVariant::Pos(Some(value))),
            // The C++ reader returns a zero value for `ParamDatatype::COUNT`.
            (ParamDatatype::Count, _) => Some(ParamVariant::Int(0)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ported from util/test/src/util/format/PrintfArgumentReaderTest.cpp.
    #[test]
    fn reads_arguments_in_order_as_requested() {
        let pos = Cell::new(0);
        let args = [
            Arg::from(1_i8),
            Arg::from(2_u8),
            Arg::from(3_i16),
            Arg::from(4_u16),
            Arg::from(5_i32),
            Arg::from(6_u32),
            Arg::from(7_i64),
            Arg::from(8_u64),
            Arg::from(9_usize),
            Arg::from(b"ten".as_slice()),
            Arg::from(&pos),
        ];
        let mut reader = SliceArgumentReader::new(&args);
        let int = |variant: Option<ParamVariant<'_>>| match variant {
            Some(ParamVariant::Int(value)) => value,
            other => panic!("expected an integer, got {other:?}"),
        };
        assert_eq!(int(reader.read_argument(ParamDatatype::Sint8)), 1);
        assert_eq!(int(reader.read_argument(ParamDatatype::Uint8)), 2);
        assert_eq!(int(reader.read_argument(ParamDatatype::Sint16)), 3);
        assert_eq!(int(reader.read_argument(ParamDatatype::Uint16)), 4);
        assert_eq!(int(reader.read_argument(ParamDatatype::Sint32)), 5);
        assert_eq!(int(reader.read_argument(ParamDatatype::Uint32)), 6);
        assert_eq!(int(reader.read_argument(ParamDatatype::Sint64)), 7);
        assert_eq!(int(reader.read_argument(ParamDatatype::Uint64)), 8);
        assert_eq!(int(reader.read_argument(ParamDatatype::VoidPtr)), 9);
        assert!(matches!(
            reader.read_argument(ParamDatatype::CharPtr),
            Some(ParamVariant::Str(Some(b"ten")))
        ));
        assert!(matches!(
            reader.read_argument(ParamDatatype::Sint32Ptr),
            Some(ParamVariant::Pos(Some(_)))
        ));
        assert!(reader.read_argument(ParamDatatype::Sint32).is_none());
    }

    #[test]
    fn negative_values_keep_their_sign_bits() {
        let args = [Arg::from(-1_i16)];
        let mut reader = SliceArgumentReader::new(&args);
        assert!(matches!(
            reader.read_argument(ParamDatatype::Sint16),
            Some(ParamVariant::Int(u64::MAX))
        ));
    }

    #[test]
    fn a_mismatched_kind_reads_as_missing() {
        let args = [Arg::from("text"), Arg::from(1_u32)];
        let mut reader = SliceArgumentReader::new(&args);
        assert!(reader.read_argument(ParamDatatype::Sint32).is_none());
        assert!(reader.read_argument(ParamDatatype::CharPtr).is_none());
    }

    #[test]
    fn count_reads_as_zero() {
        let args = [Arg::from(7_u32)];
        let mut reader = SliceArgumentReader::new(&args);
        assert!(matches!(reader.read_argument(ParamDatatype::Count), Some(ParamVariant::Int(0))));
    }

    #[test]
    fn widening_follows_the_discriminants() {
        assert_eq!(ParamDatatype::Sint8.widened(0), ParamDatatype::Sint8);
        assert_eq!(ParamDatatype::Sint8.widened(2), ParamDatatype::Sint32);
        assert_eq!(ParamDatatype::Uint8.widened(3), ParamDatatype::Uint64);
        assert_eq!(ParamDatatype::Sint32Ptr.widened(1), ParamDatatype::Count);
    }
}
