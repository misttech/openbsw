// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The ring of log entries, ported from `logger/EntryBuffer.h`.

use core::cell::{Cell, UnsafeCell};

/// A reader's position in an [`EntryBuffer`]: the index of the next entry and where it
/// starts. A fresh reference starts at the oldest entry, as does one that fell behind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EntryRef {
    idx: u32,
    read_pos: Option<usize>,
}

impl EntryRef {
    /// A reference to the oldest entry.
    pub const fn new() -> Self {
        Self { idx: 0, read_pos: None }
    }

    /// The index of the next entry to read.
    pub fn index(&self) -> u32 {
        self.idx
    }

    /// Where the next entry starts, `None` before the first read.
    pub fn read_position(&self) -> Option<usize> {
        self.read_pos
    }
}

/// A ring buffer of length-prefixed entries of at most `MAX_ENTRY` bytes; adding an entry
/// drops the oldest ones until it fits.
///
/// The buffer is written and read by different tasks, so every call must be made inside
/// the owner's critical section, as `BufferedLoggerOutput` does.
pub struct EntryBuffer<const SIZE: usize, const MAX_ENTRY: usize> {
    buffer: UnsafeCell<[u8; SIZE]>,
    first_entry: Cell<usize>,
    write: Cell<usize>,
    first_entry_index: Cell<u32>,
    write_entry_index: Cell<u32>,
    used_size: Cell<usize>,
}

// SAFETY: every access happens inside the owner's critical section (see the type
// documentation), so the cells and the byte array are never accessed concurrently.
unsafe impl<const SIZE: usize, const MAX_ENTRY: usize> Sync for EntryBuffer<SIZE, MAX_ENTRY> {}

impl<const SIZE: usize, const MAX_ENTRY: usize> EntryBuffer<SIZE, MAX_ENTRY> {
    /// An empty buffer.
    pub const fn new() -> Self {
        Self {
            buffer: UnsafeCell::new([0; SIZE]),
            first_entry: Cell::new(0),
            write: Cell::new(0),
            first_entry_index: Cell::new(0),
            write_entry_index: Cell::new(0),
            used_size: Cell::new(0),
        }
    }

    #[expect(
        clippy::mut_from_ref,
        reason = "the caller holds the owner's critical section; see the type documentation"
    )]
    fn bytes(&self) -> &mut [u8; SIZE] {
        // SAFETY: the caller holds the critical section (see the type documentation), so
        // this is the only reference to the array while it lives.
        unsafe { &mut *self.buffer.get() }
    }

    fn advance(&self, position: usize, offset: usize) -> usize {
        let moved = position + offset;
        if moved >= SIZE { moved - SIZE } else { moved }
    }

    /// Append `entry`, cut to `MAX_ENTRY` bytes, dropping the oldest entries as needed.
    pub fn add_entry(&self, entry: &[u8]) {
        let entry_size = entry.len().min(MAX_ENTRY);
        let needed_size = entry_size + 1;
        let bytes = self.bytes();
        while self.used_size.get() + needed_size > SIZE {
            self.first_entry_index.set(self.first_entry_index.get() + 1);
            let remove_size = usize::from(bytes[self.first_entry.get()]) + 1;
            self.first_entry.set(self.advance(self.first_entry.get(), remove_size));
            self.used_size.set(self.used_size.get() - remove_size);
        }
        let mut write = self.write.get();
        bytes[write] = entry_size as u8;
        write = self.advance(write, 1);
        let mut source = &entry[..entry_size];
        if write + source.len() > SIZE {
            let part = SIZE - write;
            bytes[write..].copy_from_slice(&source[..part]);
            source = &source[part..];
            write = 0;
        }
        bytes[write..write + source.len()].copy_from_slice(source);
        self.write.set(self.advance(write, source.len()));
        self.used_size.set(self.used_size.get() + needed_size);
        self.write_entry_index.set(self.write_entry_index.get() + 1);
    }

    /// Copy the entry at `entry_ref` into `out` (cut to its length) and advance the
    /// reference. Returns the number of bytes copied, 0 when no entry is left.
    pub fn next_entry(&self, out: &mut [u8], entry_ref: &mut EntryRef) -> u8 {
        let bytes = self.bytes();
        let mut entry_index = entry_ref.idx;
        let mut read_pos = entry_ref.read_pos;
        let entry_diff = entry_index.wrapping_sub(self.first_entry_index.get()) as i32;
        if read_pos.is_none() || entry_diff < 0 {
            entry_index = self.first_entry_index.get();
            read_pos = Some(self.first_entry.get());
        }
        if entry_index == self.write_entry_index.get() {
            return 0;
        }
        let mut read = read_pos.unwrap_or(0);
        let mut entry_size = bytes[read];
        read = self.advance(read, 1);
        let read_update = self.advance(read, usize::from(entry_size));
        if usize::from(entry_size) > out.len() {
            entry_size = out.len() as u8;
        }
        let mut copy_size = usize::from(entry_size);
        let mut dest = 0;
        if read + copy_size > SIZE {
            let part = SIZE - read;
            out[..part].copy_from_slice(&bytes[read..]);
            dest = part;
            read = 0;
            copy_size -= part;
        }
        out[dest..dest + copy_size].copy_from_slice(&bytes[read..read + copy_size]);
        *entry_ref = EntryRef { idx: entry_index + 1, read_pos: Some(read_update) };
        entry_size
    }
}

impl<const SIZE: usize, const MAX_ENTRY: usize> Default for EntryBuffer<SIZE, MAX_ENTRY> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use std::vec::Vec;

    struct Fixture {
        entries: Vec<Vec<u8>>,
    }

    impl Fixture {
        fn new() -> Self {
            Self { entries: Vec::new() }
        }

        fn push_entry<const S: usize, const M: usize>(
            &mut self,
            buffer: &EntryBuffer<S, M>,
            size: u8,
        ) {
            let mut value = size.wrapping_mul(self.entries.len() as u8 + 1);
            let mut entry = Vec::new();
            for _ in 0..size {
                entry.push(value);
                value = value.wrapping_add(1);
            }
            buffer.add_entry(&entry);
            self.entries.push(entry);
        }

        /// `checkNextEntry`: the next entry is the expected one, nothing is written past
        /// the output, and the size reported matches.
        fn check_next_entry<const S: usize, const M: usize>(
            &self,
            buffer: &EntryBuffer<S, M>,
            entry_ref: &mut EntryRef,
            expected: Option<usize>,
            entry_size: Option<usize>,
        ) {
            let expected_entry = expected.and_then(|index| self.entries.get(index));
            let buffer_size = entry_size.or_else(|| expected_entry.map(|e| e.len())).unwrap_or(0);
            let mut read = std::vec![0xafu8; buffer_size + 2];
            assert_eq!(
                usize::from(buffer.next_entry(&mut read[1..1 + buffer_size], entry_ref)),
                buffer_size
            );
            assert_eq!(read[0], 0xaf);
            assert_eq!(read[buffer_size + 1], 0xaf);
            if let Some(entry) = expected_entry {
                assert_eq!(&read[1..1 + buffer_size], &entry[..buffer_size]);
            }
        }
    }

    // Ported from logger/test/src/logger/EntryBufferTest.cpp.
    #[test]
    fn entry_ref_constructor_and_copy() {
        let cut = EntryRef::new();
        assert_eq!(cut.index(), 0);
        assert_eq!(cut.read_position(), None);
        let buffer: EntryBuffer<128, 64> = EntryBuffer::new();
        let mut f = Fixture::new();
        f.push_entry(&buffer, 45);
        let mut src = EntryRef::new();
        let mut out = [0u8; 10];
        buffer.next_entry(&mut out, &mut src);
        let copy = src;
        assert_eq!(copy.index(), src.index());
        assert_eq!(copy.read_position(), src.read_position());
    }

    #[test]
    fn initially_no_next_entry() {
        let cut: EntryBuffer<300, 64> = EntryBuffer::new();
        let f = Fixture::new();
        let mut entry_ref = EntryRef::new();
        f.check_next_entry(&cut, &mut entry_ref, None, None);
    }

    #[test]
    fn first_entry_is_reported_to_two_references() {
        let cut: EntryBuffer<300, 64> = EntryBuffer::new();
        let mut f = Fixture::new();
        f.push_entry(&cut, 45);
        let mut ref1 = EntryRef::new();
        f.check_next_entry(&cut, &mut ref1, Some(0), None);
        f.check_next_entry(&cut, &mut ref1, None, None);
        let mut ref2 = EntryRef::new();
        f.check_next_entry(&cut, &mut ref2, Some(0), None);
        f.check_next_entry(&cut, &mut ref2, None, None);
    }

    #[test]
    fn entries_are_cut_on_maximum_entry_size() {
        let cut: EntryBuffer<300, 64> = EntryBuffer::new();
        let mut f = Fixture::new();
        f.push_entry(&cut, 65);
        f.push_entry(&cut, 64);
        let mut entry_ref = EntryRef::new();
        f.check_next_entry(&cut, &mut entry_ref, Some(0), Some(64));
        f.check_next_entry(&cut, &mut entry_ref, Some(1), None);
        f.check_next_entry(&cut, &mut entry_ref, None, None);
    }

    #[test]
    fn wrap_around_before_new_entry() {
        let cut: EntryBuffer<128, 64> = EntryBuffer::new();
        let mut f = Fixture::new();
        f.push_entry(&cut, 63);
        f.push_entry(&cut, 63);
        f.push_entry(&cut, 63);
        let mut entry_ref = EntryRef::new();
        f.check_next_entry(&cut, &mut entry_ref, Some(1), None);
        f.check_next_entry(&cut, &mut entry_ref, Some(2), None);
        f.check_next_entry(&cut, &mut entry_ref, None, None);
    }

    #[test]
    fn wrap_around_before_length_byte_of_second_entry() {
        let cut: EntryBuffer<128, 64> = EntryBuffer::new();
        let mut f = Fixture::new();
        f.push_entry(&cut, 63);
        f.push_entry(&cut, 62);
        f.push_entry(&cut, 63);
        let mut entry_ref = EntryRef::new();
        f.check_next_entry(&cut, &mut entry_ref, Some(1), None);
        f.check_next_entry(&cut, &mut entry_ref, Some(2), None);
        f.check_next_entry(&cut, &mut entry_ref, None, None);
    }

    #[test]
    fn wrap_around_within_entry() {
        let cut: EntryBuffer<128, 64> = EntryBuffer::new();
        let mut f = Fixture::new();
        f.push_entry(&cut, 60);
        f.push_entry(&cut, 60);
        f.push_entry(&cut, 60);
        let mut entry_ref = EntryRef::new();
        f.check_next_entry(&cut, &mut entry_ref, Some(1), None);
        f.check_next_entry(&cut, &mut entry_ref, Some(2), None);
        f.check_next_entry(&cut, &mut entry_ref, None, None);
    }

    #[test]
    fn edge_cases_for_entry() {
        let cut: EntryBuffer<128, 64> = EntryBuffer::new();
        let mut f = Fixture::new();
        f.push_entry(&cut, 60);
        let mut entry_ref = EntryRef::new();
        let mut read = [0u8; 10];
        assert_eq!(cut.next_entry(&mut read, &mut entry_ref), 10);
        f.push_entry(&cut, 80);
        f.push_entry(&cut, 80);
        f.check_next_entry(&cut, &mut entry_ref, Some(3), None);
    }
}
