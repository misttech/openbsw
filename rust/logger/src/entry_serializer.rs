// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Serializing a message with its arguments, ported from `logger/EntrySerializer.h`.
//!
//! An entry is the timestamp, component index and level, the format string, and every
//! argument the format string consumes, each tagged with its datatype. Strings are copied
//! into the entry: the C++ serializer stores a pointer only for strings inside a
//! configured read-only section, and the demo configures none.
//!
//! Lengths are `u16` (the integration's `T`), timestamps `u32`.

use openbsw_util::format::{
    Arg, ArgumentReader, ParamDatatype, ParamVariant, ParamWidthOrPrecision, PrintfFormatScanner,
    SliceArgumentReader, TokenType,
};
use openbsw_util::logger::Level;
use openbsw_util::string::c_str;

const DATATYPE_CHARARRAY: u8 = ParamDatatype::Count as u8;
const DATATYPE_NONE: u8 = 0xff;
const OVERFLOW_MARK: &[u8] = b"<?>\0";

/// Writes an entry into a buffer, counting bytes beyond its end so that the size reported
/// tells how much was needed: the port of `EntryWriter`.
struct EntryWriter<'b> {
    buffer: &'b mut [u8],
    write: usize,
}

impl EntryWriter<'_> {
    fn size(&self) -> u16 {
        self.write.min(usize::from(u16::MAX)) as u16
    }

    fn write_bytes(&mut self, source: &[u8]) {
        if self.write < self.buffer.len() {
            let count = source.len().min(self.buffer.len() - self.write);
            self.buffer[self.write..self.write + count].copy_from_slice(&source[..count]);
        }
        self.write += source.len();
    }

    fn write_data(&mut self, datatype: u8, source: &[u8]) {
        self.write_bytes(&[datatype]);
        self.write_bytes(source);
    }

    /// A string as a length-prefixed, NUL-terminated byte array. When it does not fit,
    /// what fits is followed by `<?>`.
    fn write_string(&mut self, type_byte: u8, source: &[u8]) {
        self.write_bytes(&[type_byte]);
        let mut length = source.len();
        let complete_length = (length + 1) as u16;
        let header_length = core::mem::size_of::<u16>();
        if self.write + header_length + usize::from(complete_length) > self.buffer.len()
            && self.write + header_length + 5 <= self.buffer.len()
        {
            length = self.buffer.len() - self.write - header_length - 4;
            self.write_bytes(&(length as u16).to_ne_bytes());
            self.write_bytes(&source[..length]);
            self.write_bytes(OVERFLOW_MARK);
            return;
        }
        self.write_bytes(&complete_length.to_ne_bytes());
        self.write_bytes(source);
        self.write_bytes(&[0]);
    }

    fn write_param(&mut self, datatype: ParamDatatype, variant: Option<ParamVariant<'_>>) {
        let bits = match variant {
            Some(ParamVariant::Int(bits)) => bits,
            _ => 0,
        };
        match datatype {
            ParamDatatype::Uint8 => self.write_data(datatype as u8, &(bits as u8).to_ne_bytes()),
            ParamDatatype::Sint16 | ParamDatatype::Uint16 => {
                self.write_data(datatype as u8, &(bits as u16).to_ne_bytes())
            }
            ParamDatatype::Sint32 | ParamDatatype::Uint32 => {
                self.write_data(datatype as u8, &(bits as u32).to_ne_bytes())
            }
            ParamDatatype::Sint64 | ParamDatatype::Uint64 => {
                self.write_data(datatype as u8, &bits.to_ne_bytes())
            }
            ParamDatatype::VoidPtr => {
                self.write_data(datatype as u8, &(bits as usize).to_ne_bytes())
            }
            ParamDatatype::CharPtr => match variant {
                Some(ParamVariant::Str(Some(string))) => {
                    self.write_string(DATATYPE_CHARARRAY, c_str(string))
                }
                // A null pointer is stored as a null pointer.
                _ => self.write_data(ParamDatatype::CharPtr as u8, &0usize.to_ne_bytes()),
            },
            ParamDatatype::SizedCharPtr => match variant {
                Some(ParamVariant::Str(Some(string))) => {
                    self.write_string(ParamDatatype::SizedCharPtr as u8, string)
                }
                _ => self.write_string(ParamDatatype::SizedCharPtr as u8, b""),
            },
            _ => self.write_data(datatype as u8, &[]),
        }
    }
}

/// Serialize a message into `dest`; returns the number of bytes the complete entry needs,
/// which may exceed `dest`.
pub fn serialize(
    dest: &mut [u8],
    timestamp: u32,
    component_index: u8,
    level: Level,
    format: &[u8],
    args: &[Arg<'_>],
) -> u16 {
    let mut writer = EntryWriter { buffer: dest, write: 0 };
    writer.write_bytes(&timestamp.to_ne_bytes());
    writer.write_bytes(&[component_index]);
    writer.write_bytes(&[level as u8]);
    writer.write_string(DATATYPE_CHARARRAY, format);
    let mut reader = SliceArgumentReader::new(args);
    let mut scanner = PrintfFormatScanner::new(format);
    while scanner.has_token() {
        if scanner.token_type() == TokenType::Param {
            let info = *scanner.param_info();
            if info.width == ParamWidthOrPrecision::PARAM {
                let variant = reader.read_argument(ParamDatatype::Sint32);
                writer.write_param(ParamDatatype::Sint32, variant);
            }
            if info.precision == ParamWidthOrPrecision::PARAM {
                let variant = reader.read_argument(ParamDatatype::Sint32);
                writer.write_param(ParamDatatype::Sint32, variant);
            }
            let variant = reader.read_argument(info.datatype);
            writer.write_param(info.datatype, variant);
        }
        scanner.next_token();
    }
    writer.size()
}

/// Reads the arguments back out of an entry: the port of `EntryReader`.
pub struct EntryReader<'a> {
    buffer: &'a [u8],
    read: usize,
}

impl<'a> EntryReader<'a> {
    fn read_bytes(&mut self, size: usize) -> &'a [u8] {
        let start = self.read.min(self.buffer.len());
        let end = (self.read + size).min(self.buffer.len());
        self.read += size;
        &self.buffer[start..end]
    }

    fn read_u16(&mut self) -> u16 {
        let bytes = self.read_bytes(2);
        let mut value = [0u8; 2];
        value[..bytes.len()].copy_from_slice(bytes);
        u16::from_ne_bytes(value)
    }

    fn read_bits(&mut self, size: usize) -> u64 {
        let bytes = self.read_bytes(size);
        let mut value = [0u8; 8];
        value[..bytes.len()].copy_from_slice(bytes);
        u64::from_ne_bytes(value)
    }

    /// A length-prefixed string, read up to its NUL terminator: a string the writer cut
    /// declares the length of its kept part, and the `<?>` mark that follows is part of
    /// the C string the reader returns.
    fn read_char_array(&mut self) -> &'a [u8] {
        let length = usize::from(self.read_u16());
        let start = self.read.min(self.buffer.len());
        self.read += length;
        c_str(&self.buffer[start..])
    }

    fn overrun(&self) -> bool {
        self.read > self.buffer.len()
    }

    fn read_variant(&mut self) -> Option<ParamVariant<'a>> {
        let datatype = self.read_bytes(1).first().copied().unwrap_or(DATATYPE_NONE);
        let variant = match ParamDatatype::from_u8(datatype) {
            ParamDatatype::Uint8 => ParamVariant::Int(self.read_bits(1)),
            ParamDatatype::Sint16 | ParamDatatype::Uint16 => ParamVariant::Int(self.read_bits(2)),
            ParamDatatype::Sint32 | ParamDatatype::Uint32 => ParamVariant::Int(self.read_bits(4)),
            ParamDatatype::Sint64 | ParamDatatype::Uint64 => ParamVariant::Int(self.read_bits(8)),
            ParamDatatype::VoidPtr => {
                ParamVariant::Int(self.read_bits(core::mem::size_of::<usize>()))
            }
            ParamDatatype::CharPtr => {
                // Only a null pointer is stored this way.
                self.read_bits(core::mem::size_of::<usize>());
                ParamVariant::Str(None)
            }
            ParamDatatype::Count if datatype == DATATYPE_CHARARRAY => {
                ParamVariant::Str(Some(self.read_char_array()))
            }
            ParamDatatype::SizedCharPtr => ParamVariant::Str(Some(self.read_char_array())),
            _ => ParamVariant::Pos(None),
        };
        if self.overrun() { None } else { Some(variant) }
    }
}

impl<'a> ArgumentReader<'a> for EntryReader<'a> {
    fn read_argument(&mut self, _datatype: ParamDatatype) -> Option<ParamVariant<'a>> {
        self.read_variant()
    }
}

/// What [`deserialize`] calls with an entry: timestamp, component index, level, format
/// string, and the reader of the arguments.
pub type OnEntry<'f> = dyn FnMut(u32, u8, Level, &[u8], &mut dyn ArgumentReader<'_>) + 'f;

/// Deserialize an entry and hand it to `on_entry` with a reader for its arguments; an
/// entry whose header or format string does not fit is dropped.
pub fn deserialize(src: &[u8], on_entry: &mut OnEntry<'_>) {
    let mut reader = EntryReader { buffer: src, read: 0 };
    let timestamp = reader.read_bits(4) as u32;
    let component_index = reader.read_bits(1) as u8;
    let level = Level::from_u8(reader.read_bits(1) as u8);
    if let Some(ParamVariant::Str(Some(format))) = reader.read_variant() {
        on_entry(timestamp, component_index, level, format, &mut reader);
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use openbsw_util::format::StringWriter;
    use openbsw_util::stream::StringBufferOutputStream;
    use std::string::String;

    struct Fixture {
        used: usize,
    }

    impl Fixture {
        /// `serializeAndDeserialize`: serialize into `buffer_size` bytes framed by guard
        /// bytes, then deserialize what fits into `ts:component:level:message`.
        fn round_trip(
            &mut self,
            buffer_size: usize,
            timestamp: u32,
            component: u8,
            level: u8,
            format: &[u8],
            args: &[Arg<'_>],
        ) -> String {
            let mut buffer = [0xafu8; 302];
            let used = usize::from(serialize(
                &mut buffer[1..1 + buffer_size],
                timestamp,
                component,
                Level::from_u8(level),
                format,
                args,
            ));
            self.used = used.min(buffer_size);
            assert_eq!(buffer[0], 0xaf);
            assert_eq!(buffer[buffer_size + 1], 0xaf);
            let mut out = String::new();
            deserialize(
                &buffer[1..1 + self.used],
                &mut |timestamp, component, level, format, reader| {
                    let mut text = [0u8; 300];
                    let mut stream = StringBufferOutputStream::new(&mut text, b"", b"");
                    StringWriter::new(&mut stream)
                        .printf(
                            b"%d:%d:%d:",
                            &[timestamp.into(), component.into(), (level as u8).into()],
                        )
                        .vprintf(format, reader);
                    out = String::from_utf8_lossy(stream.string()).into_owned();
                },
            );
            out
        }
    }

    // Ported from logger/test/src/logger/EntrySerializerTest.cpp.
    #[test]
    fn log_with_small_buffers() {
        let mut f = Fixture { used: 0 };
        assert_eq!(f.round_trip(0, 123, 1, 2, b"test", &[]), "");
        assert_eq!(f.round_trip(1, 123, 1, 2, b"test", &[]), "");
    }

    #[test]
    fn log_with_simple_entries() {
        let mut f = Fixture { used: 0 };
        assert_eq!(f.round_trip(300, 123, 1, 2, b"simpleLog", &[]), "123:1:2:simpleLog");
        for expected in [
            "123:1:2:simpl<?>",
            "123:1:2:simp<?>",
            "123:1:2:sim<?>",
            "123:1:2:si<?>",
            "123:1:2:s<?>",
        ] {
            let size = f.used - 1;
            assert_eq!(f.round_trip(size, 123, 1, 2, b"simpleLog", &[]), expected);
        }
    }

    #[test]
    fn log_with_simple_string_arg() {
        let mut f = Fixture { used: 0 };
        assert_eq!(
            f.round_trip(300, 123, 1, 2, b"simpleArgLog(%s)", &["arg-value".into()]),
            "123:1:2:simpleArgLog(arg-value)"
        );
        for expected in [
            "123:1:2:simpleArgLog(arg-v<?>)",
            "123:1:2:simpleArgLog(arg-<?>)",
            "123:1:2:simpleArgLog(arg<?>)",
            "123:1:2:simpleArgLog(ar<?>)",
            "123:1:2:simpleArgLog(a<?>)",
            "123:1:2:simpleArgLog(<?>)",
            "123:1:2:simpleArgLog(<?>)",
        ] {
            let size = f.used - 1;
            assert_eq!(
                f.round_trip(size, 123, 1, 2, b"simpleArgLog(%s)", &["arg-value".into()]),
                expected
            );
        }
    }

    #[test]
    fn log_with_sized_string_arg() {
        let mut f = Fixture { used: 0 };
        let sized: &[u8] = &b"arg-value334"[..9];
        assert_eq!(
            f.round_trip(300, 123, 1, 2, b"simpleArgLog(%S)", &[sized.into()]),
            "123:1:2:simpleArgLog(arg-value)"
        );
        let size = f.used - 6;
        assert_eq!(
            f.round_trip(size, 123, 1, 2, b"simpleArgLog(%S)", &[sized.into()]),
            "123:1:2:simpleArgLog(<?>)"
        );
    }

    #[test]
    fn log_with_empty_sized_string_arg() {
        let mut f = Fixture { used: 0 };
        let empty: &[u8] = b"";
        assert_eq!(
            f.round_trip(300, 123, 1, 2, b"simpleArgLog(%S)", &[empty.into()]),
            "123:1:2:simpleArgLog()"
        );
        let size = f.used - 1;
        assert_eq!(
            f.round_trip(size, 123, 1, 2, b"simpleArgLog(%S)", &[empty.into()]),
            "123:1:2:simpleArgLog(<?>)"
        );
    }

    #[test]
    fn log_with_arguments() {
        let mut f = Fixture { used: 0 };
        let args: [Arg<'_>; 3] = ["arg-value".into(), 123_i32.into(), 'f'.into()];
        assert_eq!(
            f.round_trip(300, 123, 1, 2, b"simpleArgLog(%s, %d, %c)", &args),
            "123:1:2:simpleArgLog(arg-value, 123, f)"
        );
        let size = f.used - 1;
        assert_eq!(
            f.round_trip(size, 123, 1, 2, b"simpleArgLog(%s, %d, %c)", &args),
            "123:1:2:simpleArgLog(arg-value, 123, <?>)"
        );
        let size = f.used - 4;
        assert_eq!(
            f.round_trip(size, 123, 1, 2, b"simpleArgLog(%s, %d, %c)", &args),
            "123:1:2:simpleArgLog(arg-value, <?>, <?>)"
        );
    }

    #[test]
    fn printf_datatypes() {
        let mut f = Fixture { used: 0 };
        assert_eq!(
            f.round_trip(
                300,
                124,
                2,
                3,
                b"%d %hd %ld",
                &[17_i32.into(), 18_i16.into(), 19_i64.into()]
            ),
            "124:2:3:17 18 19"
        );
        assert_eq!(
            f.round_trip(
                300,
                124,
                2,
                3,
                b"%u %hu %lu",
                &[17_u32.into(), 18_u16.into(), 19_u64.into()]
            ),
            "124:2:3:17 18 19"
        );
        assert_eq!(
            f.round_trip(
                300,
                124,
                2,
                3,
                b"%c %s %s",
                &['f'.into(), "String".into(), Arg::Str(None)]
            ),
            "124:2:3:f String <NULL>"
        );
        assert_eq!(
            f.round_trip(300, 124, 2, 3, b"%p", &[0x1234_5678_usize.into()]),
            "124:2:3:12345678"
        );
        assert_eq!(f.round_trip(300, 124, 2, 3, b"%n", &[]), "124:2:3:");
        assert_eq!(
            f.round_trip(300, 124, 2, 3, b"%*.*d", &[8_i32.into(), 4_i32.into(), 17_i32.into()]),
            "124:2:3:    0017"
        );
        assert_eq!(
            f.round_trip(300, 124, 2, 3, b"%hd %d", &[(-5_i16).into(), (-7_i32).into()]),
            "124:2:3:-5 -7"
        );
    }
}
