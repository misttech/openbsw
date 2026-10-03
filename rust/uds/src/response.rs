// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The bytes a job appends to its positive response: the port of
//! `connection/PositiveResponse.h`, over the cells of the response message.

use core::cell::Cell;

/// A window on the response message's payload, filled from the front.
pub struct PositiveResponse {
    buffer: Cell<&'static [Cell<u8>]>,
    written: Cell<usize>,
    overflow: Cell<bool>,
}

// SAFETY: filled by the job processing a request, on the diagnostic context.
unsafe impl Sync for PositiveResponse {}

impl PositiveResponse {
    /// A response over no buffer.
    pub const fn new() -> Self {
        Self { buffer: Cell::new(&[]), written: Cell::new(0), overflow: Cell::new(false) }
    }

    /// Start over `buffer`.
    pub fn init(&self, buffer: &'static [Cell<u8>]) {
        self.overflow.set(false);
        self.buffer.set(buffer);
        self.reset();
    }

    /// Forget what was appended.
    pub fn reset(&self) {
        self.written.set(0);
    }

    /// Append a byte; returns whether it fit.
    pub fn append_u8(&self, data: u8) -> bool {
        self.append_data(&[data]) == 1
    }

    /// Append a big-endian 16-bit value; returns whether it fit.
    pub fn append_u16(&self, data: u16) -> bool {
        self.append_data(&data.to_be_bytes()) == 2
    }

    /// Append the low three bytes of `data`, big-endian; returns whether they fit.
    pub fn append_u24(&self, data: u32) -> bool {
        self.append_data(&data.to_be_bytes()[1..]) == 3
    }

    /// Append a big-endian 32-bit value; returns whether it fit.
    pub fn append_u32(&self, data: u32) -> bool {
        self.append_data(&data.to_be_bytes()) == 4
    }

    /// Append `data`; returns how many bytes were appended: all of them, or none when they
    /// do not fit, which marks the overflow.
    pub fn append_data(&self, data: &[u8]) -> usize {
        if self.available_data_length() < data.len() {
            self.overflow.set(true);
            return 0;
        }
        let start = self.written.get();
        for (cell, &byte) in self.buffer.get()[start..].iter().zip(data) {
            cell.set(byte);
        }
        self.written.set(start + data.len());
        data.len()
    }

    /// The buffer's size.
    pub fn maximum_length(&self) -> usize {
        self.buffer.get().len()
    }

    /// How many bytes were appended.
    pub fn length(&self) -> usize {
        self.written.get()
    }

    /// The bytes not yet written (`getData` points at the first of them).
    pub fn data(&self) -> &'static [Cell<u8>] {
        &self.buffer.get()[self.written.get()..]
    }

    /// How many bytes still fit.
    pub fn available_data_length(&self) -> usize {
        self.maximum_length() - self.written.get()
    }

    /// Count `length` bytes written through [`data`](Self::data); returns the length.
    ///
    /// # Panics
    ///
    /// When more than [`available_data_length`](Self::available_data_length) is claimed,
    /// as the C++ asserts.
    pub fn increase_data_length(&self, length: usize) -> usize {
        assert!(
            length <= self.available_data_length(),
            "length must not be greater than available data length"
        );
        self.written.set(self.written.get() + length);
        self.length()
    }

    /// Whether an append did not fit.
    pub fn is_overflow(&self) -> bool {
        self.overflow.get()
    }
}

impl Default for PositiveResponse {
    fn default() -> Self {
        Self::new()
    }
}

// Ported from PositiveResponseTest.cpp.
#[cfg(test)]
mod tests {
    extern crate std;

    use std::boxed::Box;
    use std::vec::Vec;

    use super::*;

    fn buffer(bytes: &[u8]) -> &'static [Cell<u8>] {
        Box::leak(bytes.iter().map(|&byte| Cell::new(byte)).collect::<Vec<_>>().into_boxed_slice())
    }

    fn bytes(cells: &[Cell<u8>]) -> Vec<u8> {
        cells.iter().map(Cell::get).collect()
    }

    #[test]
    fn constructor() {
        let response = PositiveResponse::new();
        assert!(response.data().is_empty());
        assert_eq!(response.length(), 0);
        assert_eq!(response.maximum_length(), 0);
        assert_eq!(response.available_data_length(), 0);
        assert!(!response.append_u8(0));
        assert!(!response.append_u16(0));
        assert!(!response.append_u24(0));
        assert!(!response.append_u32(0));
        assert_eq!(response.append_data(&[0; 10]), 0);
        assert!(response.is_overflow());
    }

    #[test]
    fn init_and_reset() {
        let response = PositiveResponse::new();
        let buffer = buffer(&[0x01, 0x02, 0x03, 0x04]);
        response.init(buffer);
        assert_eq!(bytes(buffer), [0x01, 0x02, 0x03, 0x04]);
        assert_eq!(response.data().len(), 4);
        assert_eq!(response.length(), 0);
        assert_eq!(response.maximum_length(), 4);
        assert!(response.append_u32(0x1234_5678));
        assert_eq!(bytes(buffer), [0x12, 0x34, 0x56, 0x78]);
        assert_eq!(response.length(), 4);
        response.reset();
        assert_eq!(response.data().len(), 4);
        assert_eq!(response.length(), 0);
        assert_eq!(response.maximum_length(), 4);
    }

    #[test]
    fn append_u8_u16_u24_u32() {
        let response = PositiveResponse::new();
        let two = buffer(&[0; 2]);
        response.init(two);
        assert!(response.append_u8(0x01));
        assert!(response.append_u8(0x02));
        assert!(!response.append_u8(0x02));
        assert_eq!((response.length(), bytes(two)), (2, std::vec![0x01, 0x02]));
        let four = buffer(&[0; 4]);
        response.init(four);
        assert!(response.append_u16(0x0102));
        assert!(response.append_u16(0x0304));
        assert!(!response.append_u16(0x5555));
        assert_eq!((response.length(), bytes(four)), (4, std::vec![0x01, 0x02, 0x03, 0x04]));
        let six = buffer(&[0; 6]);
        response.init(six);
        assert!(response.append_u24(0x010203));
        assert!(response.append_u24(0x040506));
        assert!(!response.append_u24(0x555555));
        assert_eq!((response.length(), bytes(six)), (6, std::vec![1, 2, 3, 4, 5, 6]));
        let eight = buffer(&[0; 8]);
        response.init(eight);
        assert!(response.append_u32(0x0102_0304));
        assert!(response.append_u32(0x0506_0708));
        assert!(!response.append_u32(0x5555_5555));
        assert_eq!((response.length(), bytes(eight)), (8, std::vec![1, 2, 3, 4, 5, 6, 7, 8]));
    }

    #[test]
    fn append_data_get_data_and_available_length() {
        let response = PositiveResponse::new();
        let four = buffer(&[0xFF; 4]);
        response.init(four);
        let data = [0x01, 0x02, 0x03, 0x04, 0x05];
        assert_eq!(response.append_data(&data[..1]), 1);
        assert_eq!(response.length(), 1);
        assert_eq!(response.append_data(&data[..3]), 3);
        assert_eq!(response.length(), 4);
        assert_eq!(bytes(four), [0x01, 0x01, 0x02, 0x03]);
        assert_eq!(response.append_data(&data[..1]), 0);
        assert_eq!(response.length(), 4);
        response.init(four);
        assert_eq!(response.available_data_length(), 4);
        for remaining in [3, 2, 1, 0] {
            response.append_u8(0x01);
            assert_eq!(response.available_data_length(), remaining);
            assert_eq!(response.data().len(), remaining);
        }
    }

    #[test]
    fn increase_data_length() {
        let response = PositiveResponse::new();
        response.init(buffer(&[0xFF; 4]));
        assert_eq!(response.increase_data_length(1), 1);
        assert_eq!(response.available_data_length(), 3);
        assert_eq!(response.increase_data_length(3), 4);
        assert_eq!(response.available_data_length(), 0);
    }

    #[test]
    #[should_panic(expected = "length must not be greater than available data length")]
    fn increase_data_length_past_the_end_panics() {
        let response = PositiveResponse::new();
        response.init(buffer(&[0xFF; 4]));
        response.increase_data_length(5);
    }
}
