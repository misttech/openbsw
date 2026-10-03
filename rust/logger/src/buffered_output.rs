// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The buffered logger output, ported from `logger/BufferedLoggerOutput.h`.

use core::cell::Cell;

use openbsw_timer::Lock;
use openbsw_util::format::{Arg, ArgumentReader};
use openbsw_util::logger::{ComponentInfo, ComponentMapping, LevelInfo, LoggerOutput};

use crate::entry_buffer::{EntryBuffer, EntryRef};
use crate::entry_serializer::{deserialize, serialize};
use crate::time::LoggerTime;

/// Receives deserialized entries: the port of `IEntryOutput`.
pub trait EntryOutput {
    /// Output the entry `entry_index`, logged at `timestamp`.
    fn output_entry(
        &self,
        entry_index: u32,
        timestamp: u32,
        component_info: &ComponentInfo,
        level_info: &LevelInfo,
        format: &[u8],
        reader: &mut dyn ArgumentReader<'_>,
    );
}

/// What a draining loop needs from a buffered output, independent of its sizes.
pub trait BufferedOutput: Sync {
    /// Output the entry after `entry_ref`, if any, and advance the reference.
    fn output_entry(&self, output: &dyn EntryOutput, entry_ref: &mut EntryRef) -> bool;
}

/// Serializes every accepted message into a ring of `SIZE` bytes, entries of at most
/// `MAX_ENTRY` bytes, stamped at log time: the port of
/// `declare::BufferedLoggerOutput<BufferSize, Lock, MaxEntrySize, ...>`.
pub struct BufferedLoggerOutput<const SIZE: usize, const MAX_ENTRY: usize, L: Lock> {
    mapping: &'static dyn ComponentMapping,
    time: &'static dyn LoggerTime,
    entries: EntryBuffer<SIZE, MAX_ENTRY>,
    _lock: core::marker::PhantomData<fn() -> L>,
}

impl<const SIZE: usize, const MAX_ENTRY: usize, L: Lock> BufferedLoggerOutput<SIZE, MAX_ENTRY, L> {
    /// A buffered output resolving names through `mapping` and stamping with `time`.
    pub const fn new(
        mapping: &'static dyn ComponentMapping,
        time: &'static dyn LoggerTime,
    ) -> Self {
        Self { mapping, time, entries: EntryBuffer::new(), _lock: core::marker::PhantomData }
    }
}

impl<const SIZE: usize, const MAX_ENTRY: usize, L: Lock> BufferedOutput
    for BufferedLoggerOutput<SIZE, MAX_ENTRY, L>
{
    fn output_entry(&self, output: &dyn EntryOutput, entry_ref: &mut EntryRef) -> bool {
        let mut entry = [0u8; MAX_ENTRY];
        let size = {
            let _lock = L::lock();
            self.entries.next_entry(&mut entry, entry_ref)
        };
        if size > 0 {
            let index = entry_ref.index();
            deserialize(
                &entry[..usize::from(size)],
                &mut |timestamp, component_index, level, format, reader| {
                    output.output_entry(
                        index,
                        timestamp,
                        &self.mapping.component_info(component_index),
                        &self.mapping.level_info(level),
                        format,
                        reader,
                    );
                },
            );
        }
        size > 0
    }
}

impl<const SIZE: usize, const MAX_ENTRY: usize, L: Lock> LoggerOutput
    for BufferedLoggerOutput<SIZE, MAX_ENTRY, L>
{
    fn log_output(
        &self,
        component_info: &ComponentInfo,
        level_info: &LevelInfo,
        format: &[u8],
        args: &[Arg<'_>],
    ) {
        let mut entry = [0u8; MAX_ENTRY];
        let timestamp = self.time.timestamp();
        let size = usize::from(serialize(
            &mut entry,
            timestamp,
            component_info.index(),
            level_info.level(),
            format,
            args,
        ))
        .min(MAX_ENTRY);
        let _lock = L::lock();
        self.entries.add_entry(&entry[..size]);
    }
}

/// A reader's position, kept across `run()` calls by the composition.
pub(crate) struct EntryCursor(pub(crate) Cell<EntryRef>);

// SAFETY: the cursor belongs to the single task that drains the log.
unsafe impl Sync for EntryCursor {}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use crate::mapping::ComponentMapping as Mapping;
    use crate::mapping::tests::{INFOS, MAP1, MAP2, MAP3};
    use crate::time::DefaultLoggerTime;
    use openbsw_util::format::StringWriter;
    use openbsw_util::logger::Level;
    use openbsw_util::stream::StringBufferOutputStream;
    use std::cell::RefCell;
    use std::string::String;
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

    static LOCKS: AtomicU32 = AtomicU32::new(0);
    /// The tests count `LOCKS`, so they run one at a time.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct CountingLock;

    impl Lock for CountingLock {
        fn lock() -> Self {
            LOCKS.fetch_add(1, Ordering::Relaxed);
            CountingLock
        }
    }

    static NOW_NS: AtomicU64 = AtomicU64::new(0);

    fn now_ns() -> u64 {
        NOW_NS.load(Ordering::Relaxed)
    }

    static TIME: DefaultLoggerTime = DefaultLoggerTime::new(&now_ns, b"%u");
    static MAPPING: Mapping<3> =
        Mapping::new(&INFOS, openbsw_util::logger::LevelInfo::default_table(), Some(&MAP1));

    struct Recorder(RefCell<String>);
    // SAFETY: single-threaded test object.
    unsafe impl Sync for Recorder {}

    impl EntryOutput for Recorder {
        fn output_entry(
            &self,
            entry_index: u32,
            timestamp: u32,
            component_info: &ComponentInfo,
            level_info: &LevelInfo,
            format: &[u8],
            reader: &mut dyn ArgumentReader<'_>,
        ) {
            let mut buffer = [0u8; 300];
            let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
            StringWriter::new(&mut stream)
                .printf(
                    b"%d %d %d %d ",
                    &[
                        entry_index.into(),
                        timestamp.into(),
                        component_info.index().into(),
                        (level_info.level() as u8).into(),
                    ],
                )
                .vprintf(format, reader);
            *self.0.borrow_mut() = String::from_utf8_lossy(stream.string()).into_owned();
        }
    }

    // Ported from logger/test/src/logger/BufferedLoggerOutputTest.cpp (testAll).
    #[test]
    fn logs_are_buffered_and_drained_one_at_a_time() {
        let _serial = SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = (&MAP2, &MAP3);
        let cut: BufferedLoggerOutput<4096, 128, CountingLock> =
            BufferedLoggerOutput::new(&MAPPING, &TIME);
        let recorder = Recorder(RefCell::new(String::new()));
        LOCKS.store(0, Ordering::Relaxed);
        NOW_NS.store(2348 * 1_000_000, Ordering::Relaxed);
        cut.log_output(
            &MAPPING.component_info(1),
            &MAPPING.level_info(Level::Debug),
            b"format string %d %s",
            &[17_i32.into(), "218439".into()],
        );
        assert_eq!(LOCKS.load(Ordering::Relaxed), 1);
        let mut entry_ref = EntryRef::new();
        assert!(cut.output_entry(&recorder, &mut entry_ref));
        assert_eq!(recorder.0.borrow().as_str(), "1 2348 1 0 format string 17 218439");
        assert_eq!(LOCKS.load(Ordering::Relaxed), 2);
        recorder.0.borrow_mut().clear();
        assert!(!cut.output_entry(&recorder, &mut entry_ref));
        assert_eq!(recorder.0.borrow().as_str(), "");
        assert_eq!(LOCKS.load(Ordering::Relaxed), 3);
    }

    // testConstructorWithPredicate: a message longer than the entry is cut with <?>.
    #[test]
    fn long_messages_are_cut() {
        let _serial = SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let cut: BufferedLoggerOutput<20, 16, CountingLock> =
            BufferedLoggerOutput::new(&MAPPING, &TIME);
        let recorder = Recorder(RefCell::new(String::new()));
        NOW_NS.store(2348 * 1_000_000, Ordering::Relaxed);
        cut.log_output(
            &MAPPING.component_info(1),
            &MAPPING.level_info(Level::Debug),
            b"very long format string that cannot fit into a 20 bytes buffer",
            &[],
        );
        let mut entry_ref = EntryRef::new();
        assert!(cut.output_entry(&recorder, &mut entry_ref));
        assert_eq!(recorder.0.borrow().as_str(), "1 2348 1 0 ver<?>");
    }
}
