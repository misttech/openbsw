// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Timestamps, ported from `logger/ILoggerTime.h` and `logger/DefaultLoggerTime.h`.

use openbsw_util::format::StringWriter;
use openbsw_util::stream::OutputStream;

/// Provides and prints the timestamps of log entries.
pub trait LoggerTime: Sync {
    /// The current timestamp.
    fn timestamp(&self) -> u32;

    /// Print `timestamp`.
    fn format_timestamp(&self, stream: &mut dyn OutputStream, timestamp: u32);
}

/// Milliseconds since boot from a nanosecond clock, printed with a printf format:
/// the port of `DefaultLoggerTime<>` over `etl::chrono::high_resolution_clock`.
pub struct DefaultLoggerTime {
    now_ns: &'static (dyn Fn() -> u64 + Sync),
    time_format: &'static [u8],
}

impl DefaultLoggerTime {
    /// A time source reading `now_ns` and printing the milliseconds with `time_format`
    /// (`b"%u"` in the demo).
    pub const fn new(
        now_ns: &'static (dyn Fn() -> u64 + Sync),
        time_format: &'static [u8],
    ) -> Self {
        Self { now_ns, time_format }
    }
}

impl LoggerTime for DefaultLoggerTime {
    fn timestamp(&self) -> u32 {
        ((self.now_ns)() / 1_000_000) as u32
    }

    fn format_timestamp(&self, stream: &mut dyn OutputStream, timestamp: u32) {
        StringWriter::new(stream).printf(self.time_format, &[timestamp.into()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicU64, Ordering};
    use openbsw_util::stream::StringBufferOutputStream;

    static NOW: AtomicU64 = AtomicU64::new(0);

    fn now_ns() -> u64 {
        NOW.load(Ordering::Relaxed)
    }

    // Ported from logger/test/src/logger/DefaultLoggerTimeTest.cpp.
    #[test]
    fn now_is_taken_from_the_clock_in_milliseconds() {
        let cut = DefaultLoggerTime::new(&now_ns, b"%u");
        NOW.store(123_000_000, Ordering::Relaxed);
        assert_eq!(cut.timestamp(), 123);
        NOW.store(123_999_999, Ordering::Relaxed);
        assert_eq!(cut.timestamp(), 123);
    }

    #[test]
    fn time_is_formatted_as_expected() {
        let cut = DefaultLoggerTime::new(&now_ns, b"%04u");
        let mut buffer = [0u8; 40];
        let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
        cut.format_timestamp(&mut stream, 127);
        assert_eq!(stream.string(), b"0127");
    }
}
