// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Entry formatting, ported from `logger/IEntryFormatter.h` and
//! `loggerIntegration`'s `ConsoleEntryFormatter.h`.

use openbsw_util::format::{ArgumentReader, StringWriter, Vt100AttributedStringFormatter};
use openbsw_util::logger::{ComponentInfo, LevelInfo};
use openbsw_util::stream::OutputStream;

use crate::time::LoggerTime;

/// Formats one entry onto a stream.
pub trait EntryFormatter: Sync {
    /// Format the entry `entry_index`, logged at `timestamp`.
    #[expect(clippy::too_many_arguments, reason = "mirrors the C++ interface")]
    fn format_entry(
        &self,
        stream: &mut dyn OutputStream,
        entry_index: u32,
        timestamp: u32,
        component_info: &ComponentInfo,
        level_info: &LevelInfo,
        format: &[u8],
        reader: &mut dyn ArgumentReader<'_>,
    );
}

/// The console line: `<timestamp>: <name>: <component>: <level>: <message>`, with the
/// component and level names in their VT100 colors.
pub struct ConsoleEntryFormatter {
    time: &'static dyn LoggerTime,
    name: &'static [u8],
}

impl ConsoleEntryFormatter {
    /// A formatter printing timestamps through `time` and the core name `name`.
    pub const fn new(time: &'static dyn LoggerTime, name: &'static [u8]) -> Self {
        Self { time, name }
    }
}

impl EntryFormatter for ConsoleEntryFormatter {
    fn format_entry(
        &self,
        stream: &mut dyn OutputStream,
        _entry_index: u32,
        timestamp: u32,
        component_info: &ComponentInfo,
        level_info: &LevelInfo,
        format: &[u8],
        reader: &mut dyn ArgumentReader<'_>,
    ) {
        let vt100 = Vt100AttributedStringFormatter::new();
        self.time.format_timestamp(stream, timestamp);
        StringWriter::new(stream)
            .write(b": ")
            .write(self.name)
            .write(b": ")
            .apply(&vt100.write(component_info.name()))
            .write(b": ")
            .apply(&vt100.write(level_info.name()))
            .write(b": ")
            .vprintf(format, reader);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::DefaultLoggerTime;
    use openbsw_util::format::{Color, SliceArgumentReader, StringAttributes};
    use openbsw_util::logger::{Level, PlainComponentInfo};
    use openbsw_util::stream::StringBufferOutputStream;

    fn zero() -> u64 {
        0
    }

    static TIME: DefaultLoggerTime = DefaultLoggerTime::new(&zero, b"%u");
    static LIFECYCLE: PlainComponentInfo =
        PlainComponentInfo::new(b"LIFECYCLE", StringAttributes::color(Color::DarkGray));
    static DEMO: PlainComponentInfo =
        PlainComponentInfo::new(b"DEMO", StringAttributes::color(Color::DefaultColor));

    /// The demo's lines, byte for byte.
    #[test]
    fn formats_the_demo_lines() {
        let cut = ConsoleEntryFormatter::new(&TIME, b"Core0");
        let mut buffer = [0u8; 120];
        let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
        cut.format_entry(
            &mut stream,
            7,
            0,
            &ComponentInfo::new(0, Some(&LIFECYCLE)),
            &LevelInfo::new(Some(&LevelInfo::default_table()[Level::Info as usize])),
            b"%s level %d",
            &mut SliceArgumentReader::new(&["Initialize".into(), 1_u8.into()]),
        );
        assert_eq!(
            stream.string(),
            b"0: Core0: \x1b[90mLIFECYCLE\x1b[0m: INFO: Initialize level 1"
        );
        let mut buffer = [0u8; 120];
        let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
        cut.format_entry(
            &mut stream,
            8,
            1011,
            &ComponentInfo::new(1, Some(&DEMO)),
            &LevelInfo::new(Some(&LevelInfo::default_table()[Level::Debug as usize])),
            b"Sending frame %d",
            &mut SliceArgumentReader::new(&[0_u32.into()]),
        );
        assert_eq!(stream.string(), b"1011: Core0: DEMO: DEBUG: Sending frame 0");
        let mut buffer = [0u8; 120];
        let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
        cut.format_entry(
            &mut stream,
            9,
            5,
            &ComponentInfo::new(1, Some(&DEMO)),
            &LevelInfo::new(Some(&LevelInfo::default_table()[Level::Warn as usize])),
            b"x",
            &mut SliceArgumentReader::new(&[]),
        );
        assert_eq!(stream.string(), b"5: Core0: DEMO: \x1b[33;1mWARN\x1b[0m: x");
    }
}
