// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Writing entries to the console, ported from `logger/ConsoleEntryOutput.h`.

use openbsw_util::format::ArgumentReader;
use openbsw_util::logger::{ComponentInfo, LevelInfo};
use openbsw_util::stream::{NormalizeLfOutputStream, OutputStream, Stdio, StdoutStream};

use crate::buffered_output::EntryOutput;
use crate::formatter::EntryFormatter;

/// Formats each entry onto the console, ending it with `\n`, which the stream turns into
/// `\r\n`.
pub struct ConsoleEntryOutput {
    stdio: &'static dyn Stdio,
    formatter: &'static dyn EntryFormatter,
}

impl ConsoleEntryOutput {
    /// An output writing through `stdio` what `formatter` produces.
    pub const fn new(stdio: &'static dyn Stdio, formatter: &'static dyn EntryFormatter) -> Self {
        Self { stdio, formatter }
    }
}

impl EntryOutput for ConsoleEntryOutput {
    fn output_entry(
        &self,
        entry_index: u32,
        timestamp: u32,
        component_info: &ComponentInfo,
        level_info: &LevelInfo,
        format: &[u8],
        reader: &mut dyn ArgumentReader<'_>,
    ) {
        let mut stdout = StdoutStream::new(self.stdio);
        let mut stream = NormalizeLfOutputStream::new(&mut stdout, None);
        self.formatter.format_entry(
            &mut stream,
            entry_index,
            timestamp,
            component_info,
            level_info,
            format,
            reader,
        );
        stream.write(b'\n');
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use openbsw_util::format::{ParamDatatype, ParamVariant, StringAttributes, StringWriter};
    use openbsw_util::logger::{Level, PlainComponentInfo, PlainLevelInfo};
    use std::cell::RefCell;
    use std::vec::Vec;

    struct Fixture {
        out: RefCell<Vec<u8>>,
    }
    // SAFETY: single-threaded test object.
    unsafe impl Sync for Fixture {}

    impl Stdio for Fixture {
        fn get_byte(&self) -> i32 {
            -1
        }
        fn put_byte(&self, byte: u8) {
            self.out.borrow_mut().push(byte);
        }
    }

    impl EntryFormatter for Fixture {
        fn format_entry(
            &self,
            stream: &mut dyn OutputStream,
            entry_index: u32,
            timestamp: u32,
            component_info: &ComponentInfo,
            level_info: &LevelInfo,
            format: &[u8],
            reader: &mut dyn ArgumentReader<'_>,
        ) {
            StringWriter::new(stream)
                .printf(
                    b"%d %d %d %s ",
                    &[
                        entry_index.into(),
                        timestamp.into(),
                        component_info.index().into(),
                        level_info.name().string().into(),
                    ],
                )
                .vprintf(format, reader);
        }
    }

    struct Args(u32);

    impl ArgumentReader<'static> for Args {
        fn read_argument(&mut self, _datatype: ParamDatatype) -> Option<ParamVariant<'static>> {
            self.0 += 1;
            match self.0 {
                1 => Some(ParamVariant::Int(83743)),
                2 => Some(ParamVariant::Str(Some(b"String"))),
                _ => None,
            }
        }
    }

    static FIXTURE: Fixture = Fixture { out: RefCell::new(Vec::new()) };
    static COMPONENT: PlainComponentInfo = PlainComponentInfo::new(
        b"Component_Name",
        StringAttributes::new(
            openbsw_util::format::Color::DefaultColor,
            0,
            openbsw_util::format::Color::DefaultColor,
        ),
    );
    static LEVEL: PlainLevelInfo = PlainLevelInfo::new(
        b"Level_Name",
        StringAttributes::new(
            openbsw_util::format::Color::DefaultColor,
            0,
            openbsw_util::format::Color::DefaultColor,
        ),
        Level::Debug,
    );

    // Ported from logger/test/src/logger/ConsoleEntryOutputTest.cpp.
    #[test]
    fn writes_the_formatted_entry_with_crlf() {
        let cut = ConsoleEntryOutput::new(&FIXTURE, &FIXTURE);
        cut.output_entry(
            15,
            15343,
            &ComponentInfo::new(16, Some(&COMPONENT)),
            &LevelInfo::new(Some(&LEVEL)),
            b"Format string %d %s",
            &mut Args(0),
        );
        assert_eq!(
            FIXTURE.out.borrow().as_slice(),
            b"15 15343 16 Level_Name Format string 83743 String\r\n"
        );
    }
}
