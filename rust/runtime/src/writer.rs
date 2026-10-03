// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The console table of statistics, ported from `StatisticsWriter.h` and `.cpp`.

use openbsw_util::format::{Arg, StringWriter};

use crate::container::{HasStatistics, StatisticsIterator};

/// What a column call writes: its title, a line of dashes, or its value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The title row.
    Header,
    /// The dashes under the titles.
    Line,
    /// A value row.
    Value,
}

/// How clock ticks become microseconds.
#[derive(Clone, Copy)]
pub enum TickConversion<'f> {
    /// Divide by this many ticks per microsecond.
    TicksPerUs(u32),
    /// Ask the platform (`systemTicksToTimeUs`), as the C++ does for 0 ticks per
    /// microsecond.
    Function(&'f dyn Fn(u32) -> u32),
}

impl TickConversion<'_> {
    fn to_us(self, ticks: u32) -> u32 {
        match self {
            Self::TicksPerUs(ticks_per_us) => ticks / ticks_per_us,
            Self::Function(convert) => convert(ticks),
        }
    }
}

const LINE_CHAR: u8 = b'-';

/// Writes columns of a statistics table through a [`StringWriter`].
pub struct StatisticsWriter<'w, 's, 'f> {
    writer: &'w mut StringWriter<'s>,
    total_runtime: u32,
    ticks: TickConversion<'f>,
    mode: Mode,
    is_line_start: bool,
}

impl<'w, 's, 'f> StatisticsWriter<'w, 's, 'f> {
    /// A writer whose percentages are of `total_runtime`.
    pub fn new(
        writer: &'w mut StringWriter<'s>,
        total_runtime: u32,
        ticks: TickConversion<'f>,
    ) -> Self {
        Self { writer, total_runtime, ticks, mode: Mode::Value, is_line_start: true }
    }

    /// Choose what the column calls write.
    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }

    /// End the row.
    pub fn write_eol(&mut self) {
        self.writer.write(b"\n");
        self.is_line_start = true;
    }

    /// A left-aligned text column.
    pub fn write_text(&mut self, title: &[u8], min_width: u32, text: &[u8]) {
        if let Some(width) = self.value_column(title, min_width, 0, true) {
            self.writer.printf(b"%-*s", &[width.into(), Arg::Str(Some(text))]);
        }
    }

    /// A right-aligned number column.
    pub fn write_number(&mut self, title: &[u8], min_width: u32, value: u32) {
        if let Some(width) = self.value_column(title, min_width, 0, false) {
            self.writer.printf(b"%*d", &[width.into(), value.into()]);
        }
    }

    /// A run time in microseconds.
    pub fn write_runtime(&mut self, title: &[u8], min_width: u32, runtime: u32) {
        if let Some(width) = self.value_column(title, min_width, 3, false) {
            self.writer.printf(b"%*d us", &[width.into(), self.ticks.to_us(runtime).into()]);
        }
    }

    /// A run time in milliseconds.
    pub fn write_runtime_ms(&mut self, title: &[u8], min_width: u32, runtime: u32) {
        if let Some(width) = self.value_column(title, min_width, 3, false) {
            self.writer
                .printf(b"%*d ms", &[width.into(), (self.ticks.to_us(runtime) / 1000).into()]);
        }
    }

    /// A run time as a percentage of the total.
    pub fn write_runtime_percentage(&mut self, title: &[u8], runtime: u32) {
        self.write_percentage(title, runtime, self.total_runtime);
    }

    /// `value` as a percentage of `total`, with two decimals.
    pub fn write_percentage(&mut self, title: &[u8], value: u32, total: u32) {
        if let Some(width) = self.value_column(title, 6, 3, false) {
            let percentage =
                if total != 0 { (u64::from(value) * 10000 / u64::from(total)) as u32 } else { 0 };
            self.writer.printf(
                b" %*s%3d.%02d %%",
                &[
                    (width - 6).into(),
                    Arg::Str(Some(b"")),
                    (percentage / 100).into(),
                    (percentage % 100).into(),
                ],
            );
        }
    }

    /// One row: the name, then the columns `format` writes, then the end of the row.
    pub fn format_statistics_line<S>(
        &mut self,
        format: &dyn Fn(&mut Self, &S),
        title: &[u8],
        min_width: u32,
        name: &[u8],
        statistics: &S,
    ) {
        self.write_text(title, min_width, name);
        format(self, statistics);
        self.write_eol();
    }

    /// A table: the titles, the dashes, and one row per named entry.
    pub fn format_statistics_group<E>(
        &mut self,
        format: &dyn Fn(&mut Self, &E::Statistics),
        title: &[u8],
        min_width: u32,
        mut iterator: StatisticsIterator<E>,
    ) where
        E: HasStatistics + 'static,
        E::Statistics: Default,
    {
        let placeholder = E::Statistics::default();
        self.set_mode(Mode::Header);
        self.format_statistics_line(format, title, min_width, b"", &placeholder);
        self.set_mode(Mode::Line);
        self.format_statistics_line(format, title, min_width, b"", &placeholder);
        self.set_mode(Mode::Value);
        while iterator.has_value() {
            let name = iterator.name().unwrap_or(b"");
            self.format_statistics_line(
                format,
                title,
                min_width,
                name,
                iterator.statistics().statistics(),
            );
            iterator.next();
        }
    }

    /// Writes the column separator and, in header and line mode, the column itself;
    /// returns the width of the value to write in value mode (`handleDefaultMode`).
    fn value_column(
        &mut self,
        title: &[u8],
        min_width: u32,
        padding: u32,
        left_aligned: bool,
    ) -> Option<u32> {
        if self.is_line_start {
            self.is_line_start = false;
        } else {
            self.writer.write_char(if self.mode == Mode::Line { LINE_CHAR } else { b' ' });
        }
        let column_width = (min_width + padding).max(title.len() as u32);
        match self.mode {
            Mode::Header => {
                let format: &[u8] = if left_aligned { b"%-*s" } else { b"%*s" };
                self.writer.printf(format, &[column_width.into(), Arg::Str(Some(title))]);
                None
            }
            Mode::Line => {
                for _ in 0..column_width {
                    self.writer.write_char(LINE_CHAR);
                }
                None
            }
            Mode::Value => Some(column_width - padding),
        }
    }
}

// Ported from runtime/test/src/StatisticsWriterTest.cpp.
#[cfg(test)]
mod tests {
    extern crate std;

    use std::boxed::Box;

    use openbsw_util::stream::StringBufferOutputStream;

    use super::*;
    use crate::container::StatisticsContainer;
    use crate::statistics::{RuntimeStatistics, Statistics};

    fn write_statistics(writer: &mut StatisticsWriter<'_, '_, '_>) {
        writer.write_text(b"text-header", 9, b"ABCDEFG");
        writer.write_text(b"text 2", 8, b"abcdef");
        writer.write_number(b"long-number", 4, 12345);
        writer.write_number(b"#", 6, 12345);
        writer.write_runtime(b"us-header", 5, 5000);
        writer.write_runtime(b"us", 6, 7500);
        writer.write_runtime_percentage(b"percentage", 8123);
        writer.write_runtime_percentage(b"pct", 10000);
        writer.write_percentage(b"Percentage", 3483, 10000);
        writer.write_percentage(b"Pct", 720, 10000);
        writer.write_eol();
    }

    fn with_writer(
        total: u32,
        ticks: TickConversion<'_>,
        mode: Mode,
        f: impl FnOnce(&mut StatisticsWriter<'_, '_, '_>),
    ) -> std::string::String {
        let mut buffer = [0_u8; 300];
        let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
        {
            let mut writer = StringWriter::new(&mut stream);
            let mut cut = StatisticsWriter::new(&mut writer, total, ticks);
            cut.set_mode(mode);
            f(&mut cut);
        }
        core::str::from_utf8(stream.string()).unwrap().into()
    }

    #[test]
    fn header() {
        assert_eq!(
            with_writer(10000, TickConversion::TicksPerUs(50), Mode::Header, write_statistics),
            "text-header text 2   long-number      # us-header        us percentage       pct Percentage       Pct\n"
        );
    }

    #[test]
    fn line() {
        assert_eq!(
            with_writer(10000, TickConversion::TicksPerUs(50), Mode::Line, write_statistics),
            "-----------------------------------------------------------------------------------------------------\n"
        );
    }

    #[test]
    fn values() {
        assert_eq!(
            with_writer(10000, TickConversion::TicksPerUs(50), Mode::Value, write_statistics),
            "ABCDEFG     abcdef         12345  12345    100 us    150 us    81.23 %  100.00 %    34.83 %    7.20 %\n"
        );
    }

    #[test]
    fn percentage_without_runtime() {
        assert_eq!(
            with_writer(0, TickConversion::TicksPerUs(50), Mode::Value, |cut| cut
                .write_percentage(b"test", 125, 0)),
            "   0.00 %"
        );
        assert_eq!(
            with_writer(0, TickConversion::TicksPerUs(50), Mode::Value, |cut| cut
                .write_runtime_percentage(b"test", 125)),
            "   0.00 %"
        );
    }

    fn format(writer: &mut StatisticsWriter<'_, '_, '_>, statistics: &RuntimeStatistics) {
        writer.write_runtime_percentage(b"%", statistics.total_runtime());
        writer.write_runtime_ms(b"min", 6, statistics.min_runtime());
        writer.write_runtime_ms(b"max", 6, statistics.max_runtime());
        writer.write_runtime_ms(b"avg", 6, statistics.average_runtime());
    }

    #[test]
    fn format_group() {
        let statistics: &'static [RuntimeStatistics; 4] =
            Box::leak(Box::new(core::array::from_fn(|_| RuntimeStatistics::new())));
        statistics[0].add_run(0, 300_000, 0);
        statistics[1].add_run(0, 250_000, 0);
        statistics[1].add_run(0, 420_000, 0);
        statistics[3].add_run(0, 10_000, 0);
        statistics[3].add_run(0, 1_010_000, 0);
        fn name(idx: usize) -> Option<&'static [u8]> {
            [Some(&b"first"[..]), Some(b"middle"), None, Some(b"last")][idx]
        }
        let container = StatisticsContainer::new(statistics, Some(&name));
        let output = with_writer(10_000_000, TickConversion::TicksPerUs(50), Mode::Value, |cut| {
            cut.format_statistics_group(&format, b"task", 9, container.iter());
        });
        assert_eq!(
            output,
            "task              %       min       max       avg\n\
             -------------------------------------------------\n\
             first        3.00 %      6 ms      6 ms      6 ms\n\
             middle       6.70 %      5 ms      8 ms      6 ms\n\
             last        10.20 %      0 ms     20 ms     10 ms\n"
        );
    }

    #[test]
    fn builtin_ticks_converter() {
        let convert = |ticks: u32| ticks / 1000;
        assert_eq!(
            with_writer(0, TickConversion::Function(&convert), Mode::Value, |cut| cut
                .write_runtime(b"test", 4, 5000)),
            "   5 us"
        );
        assert_eq!(
            with_writer(0, TickConversion::Function(&convert), Mode::Value, |cut| cut
                .write_runtime_ms(b"test", 4, 6_000_000)),
            "   6 ms"
        );
    }
}
