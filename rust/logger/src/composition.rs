// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The demo's logger wiring, ported from `loggerIntegration`'s `LoggerComposition`.

use core::cell::Cell;

use openbsw_util::logger::LoggerOutput;
use openbsw_util::stream::Stdio;

use crate::buffered_output::{BufferedOutput, EntryCursor};
use crate::console_output::ConsoleEntryOutput;
use crate::entry_buffer::EntryRef;
use crate::formatter::ConsoleEntryFormatter;
use crate::time::LoggerTime;

/// Drains a buffered output onto the console one entry per `run()`.
///
/// C++ builds the time source, formatter, console output and buffered output as members;
/// here they are separate `static`s the composition refers to, since a `const`
/// constructor cannot point at its own fields.
pub struct LoggerComposition {
    buffered: &'static dyn BufferedOutput,
    logger_output: &'static dyn LoggerOutput,
    formatter: ConsoleEntryFormatter,
    stdio: &'static dyn Stdio,
    cursor: EntryCursor,
}

impl LoggerComposition {
    /// A composition draining `buffered` (also the facade's output, `logger_output`) onto
    /// `stdio`, printing timestamps through `time` and the core name `name`.
    pub const fn new(
        buffered: &'static dyn BufferedOutput,
        logger_output: &'static dyn LoggerOutput,
        time: &'static dyn LoggerTime,
        name: &'static [u8],
        stdio: &'static dyn Stdio,
    ) -> Self {
        Self {
            buffered,
            logger_output,
            formatter: ConsoleEntryFormatter::new(time, name),
            stdio,
            cursor: EntryCursor(Cell::new(EntryRef::new())),
        }
    }

    /// Start logging: `config_start` connects the facade to the buffered output.
    pub fn start(&self, config_start: impl FnOnce(&'static dyn LoggerOutput)) {
        config_start(self.logger_output);
    }

    /// Output the next buffered entry, if any.
    pub fn run(&'static self) {
        self.output_one();
    }

    /// Output every buffered entry, then run `config_stop`.
    pub fn stop(&'static self, config_stop: impl FnOnce()) {
        while self.output_one() {}
        config_stop();
    }

    fn output_one(&'static self) -> bool {
        let console = ConsoleEntryOutput::new(self.stdio, &self.formatter);
        let mut entry_ref = self.cursor.0.get();
        let output = self.buffered.output_entry(&console, &mut entry_ref);
        self.cursor.0.set(entry_ref);
        output
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use crate::buffered_output::BufferedLoggerOutput;
    use crate::config::ComponentConfig;
    use crate::mapping::{ComponentMapping, MappingInfo};
    use crate::time::DefaultLoggerTime;
    use openbsw_timer::NoLock;
    use openbsw_util::format::{Color, StringAttributes};
    use openbsw_util::logger::{Level, LevelInfo, LoggerComponent};
    use openbsw_util::{log_debug, log_info};
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::vec::Vec;

    static NOW_NS: AtomicU64 = AtomicU64::new(0);

    fn now_ns() -> u64 {
        NOW_NS.load(Ordering::Relaxed)
    }

    struct Console(RefCell<Vec<u8>>);
    // SAFETY: single-threaded test object.
    unsafe impl Sync for Console {}

    impl Stdio for Console {
        fn get_byte(&self) -> i32 {
            255
        }
        fn put_byte(&self, byte: u8) {
            self.0.borrow_mut().push(byte);
        }
    }

    static LIFECYCLE: LoggerComponent = LoggerComponent::new();
    static DEMO: LoggerComponent = LoggerComponent::new();
    static INFOS: [MappingInfo; 2] = [
        MappingInfo::new(
            Level::Debug,
            &LIFECYCLE,
            b"LIFECYCLE",
            StringAttributes::color(Color::DarkGray),
        ),
        MappingInfo::new(
            Level::Debug,
            &DEMO,
            b"DEMO",
            StringAttributes::color(Color::DefaultColor),
        ),
    ];
    static MAPPING: ComponentMapping<2> =
        ComponentMapping::new(&INFOS, LevelInfo::default_table(), None);
    static CONFIG: ComponentConfig<2> = ComponentConfig::new(&MAPPING);
    static TIME: DefaultLoggerTime = DefaultLoggerTime::new(&now_ns, b"%u");
    static BUFFERED: BufferedLoggerOutput<8192, 128, NoLock> =
        BufferedLoggerOutput::new(&MAPPING, &TIME);
    static CONSOLE: Console = Console(RefCell::new(Vec::new()));
    static COMPOSITION: LoggerComposition =
        LoggerComposition::new(&BUFFERED, &BUFFERED, &TIME, b"Core0", &CONSOLE);

    /// The demo's logger.cpp wiring: entries keep the time they were logged and come out
    /// one per run(), as `<ms>: Core0: <component>: <level>: <message>\r\n`.
    #[test]
    fn drains_one_entry_per_run_with_the_console_format() {
        let _serial = crate::LOGGER_SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        COMPOSITION.start(|output| CONFIG.start(output));
        NOW_NS.store(0, Ordering::Relaxed);
        log_info!(LIFECYCLE, b"%s level %d", "Initialize", 1_u8);
        NOW_NS.store(1_011 * 1_000_000, Ordering::Relaxed);
        log_debug!(DEMO, b"Sending frame %d", 0_u32);
        NOW_NS.store(5_000 * 1_000_000, Ordering::Relaxed);
        COMPOSITION.run();
        assert_eq!(
            CONSOLE.0.borrow().as_slice(),
            b"0: Core0: \x1b[90mLIFECYCLE\x1b[0m: INFO: Initialize level 1\r\n"
        );
        COMPOSITION.run();
        assert_eq!(
            CONSOLE.0.borrow().as_slice(),
            b"0: Core0: \x1b[90mLIFECYCLE\x1b[0m: INFO: Initialize level 1\r\n1011: Core0: DEMO: DEBUG: Sending frame 0\r\n"
        );
        COMPOSITION.run();
        assert_eq!(CONSOLE.0.borrow().len(), 99);
        log_info!(DEMO, b"a");
        log_info!(DEMO, b"b");
        COMPOSITION.stop(|| CONFIG.shutdown());
        assert!(
            CONSOLE
                .0
                .borrow()
                .ends_with(b"5000: Core0: DEMO: INFO: a\r\n5000: Core0: DEMO: INFO: b\r\n")
        );
    }
}
