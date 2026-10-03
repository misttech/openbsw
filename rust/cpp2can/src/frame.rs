// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! A classic CAN frame, ported from `canframes/CANFrame.h` and `CANFrame.cpp` (without
//! `CPP2CAN_USE_64_BYTE_FRAMES`).

use crate::can_id;

/// A CAN frame: an id (see [`can_id`]), up to [`CanFrame::MAX_FRAME_LENGTH`] bytes of
/// payload, and a timestamp the transceiver sets.
///
/// Two frames are equal when their ids, lengths and payload bytes up to the length are;
/// the timestamp does not count.
#[derive(Clone, Copy, Debug)]
pub struct CanFrame {
    id: u32,
    timestamp: u32,
    payload: [u8; Self::MAX_FRAME_LENGTH as usize],
    payload_length: u8,
}

impl CanFrame {
    /// The sender mask.
    pub const SENDER_MASK: u8 = 0xFF;
    /// The bits a frame carries besides its payload.
    pub const CAN_OVERHEAD_BITS: u8 = 47;
    /// The longest payload.
    pub const MAX_FRAME_LENGTH: u8 = 8;
    /// The largest base id.
    pub const MAX_FRAME_ID: u32 = 0x7FF;
    /// The largest extended id.
    pub const MAX_FRAME_ID_EXTENDED: u32 = 0x1FFF_FFFF;

    /// An empty frame with id 0 and a zeroed payload.
    pub const fn new() -> Self {
        Self {
            id: 0,
            timestamp: 0,
            payload: [0; Self::MAX_FRAME_LENGTH as usize],
            payload_length: 0,
        }
    }

    /// An empty frame with `id` and a payload of `0xFF` bytes.
    pub const fn with_id(id: u32) -> Self {
        Self {
            id,
            timestamp: 0,
            payload: [0xFF; Self::MAX_FRAME_LENGTH as usize],
            payload_length: 0,
        }
    }

    /// A frame with `id` and `payload`.
    ///
    /// # Panics
    ///
    /// If `payload` is longer than [`Self::MAX_FRAME_LENGTH`] (the C++ asserts).
    pub const fn from_payload(id: u32, payload: &[u8]) -> Self {
        assert!(
            payload.len() <= Self::MAX_FRAME_LENGTH as usize,
            "CAN frame length must be smaller than maximum length"
        );
        let mut frame = Self::new();
        frame.id = id;
        frame.copy_payload(payload);
        frame
    }

    /// A frame with raw identifier `raw_id`, base or extended, and `payload`.
    ///
    /// # Panics
    ///
    /// If `raw_id` exceeds [`Self::MAX_FRAME_ID_EXTENDED`] or `payload` is longer than
    /// [`Self::MAX_FRAME_LENGTH`] (the C++ asserts).
    pub const fn from_raw_id(raw_id: u32, payload: &[u8], is_extended_id: bool) -> Self {
        assert!(
            raw_id <= Self::MAX_FRAME_ID_EXTENDED,
            "CAN frame id must be smaller than the maximum allowed one"
        );
        Self::from_payload(can_id::id(raw_id, is_extended_id), payload)
    }

    /// The id.
    pub const fn id(&self) -> u32 {
        self.id
    }

    /// Set the id.
    pub const fn set_id(&mut self, id: u32) {
        self.id = id;
    }

    /// The payload bytes up to the payload length.
    pub fn payload(&self) -> &[u8] {
        &self.payload[..usize::from(self.payload_length)]
    }

    /// The whole payload buffer, for writing bytes in place (`getPayload()` in C++).
    pub const fn payload_mut(&mut self) -> &mut [u8; Self::MAX_FRAME_LENGTH as usize] {
        &mut self.payload
    }

    /// Copy `payload` in and set the length to its size.
    ///
    /// # Panics
    ///
    /// If `payload` is longer than [`Self::MAX_FRAME_LENGTH`] (the C++ asserts).
    pub const fn set_payload(&mut self, payload: &[u8]) {
        assert!(
            payload.len() <= Self::MAX_FRAME_LENGTH as usize,
            "CAN frame length must be smaller than maximum length"
        );
        self.copy_payload(payload);
    }

    /// Set the payload length; a length over the maximum is ignored.
    pub const fn set_payload_length(&mut self, length: u8) {
        if length <= Self::MAX_FRAME_LENGTH {
            self.payload_length = length;
        }
    }

    /// The payload length.
    pub const fn payload_length(&self) -> u8 {
        self.payload_length
    }

    /// The longest payload.
    pub const fn max_payload_length() -> u8 {
        Self::MAX_FRAME_LENGTH
    }

    /// The timestamp.
    pub const fn timestamp(&self) -> u32 {
        self.timestamp
    }

    /// Set the timestamp.
    pub const fn set_timestamp(&mut self, timestamp: u32) {
        self.timestamp = timestamp;
    }

    const fn copy_payload(&mut self, payload: &[u8]) {
        let mut i = 0;
        while i < payload.len() {
            self.payload[i] = payload[i];
            i += 1;
        }
        self.payload_length = payload.len() as u8;
    }
}

impl Default for CanFrame {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for CanFrame {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.payload() == other.payload()
    }
}

impl Eq for CanFrame {}

// Ported from cpp2can/test/src/can/canframes/CANFrameTest.cpp.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_constructor() {
        let frame = CanFrame::new();
        assert_eq!(frame.id(), 0);
        assert_eq!(frame.payload_length(), 0);
    }

    #[test]
    #[should_panic(expected = "CAN frame length")]
    fn init_constructor_rejects_a_long_payload() {
        let payload = [0; CanFrame::MAX_FRAME_LENGTH as usize + 1];
        let _ = CanFrame::from_payload(0x345, &payload);
    }

    #[test]
    #[should_panic(expected = "CAN frame length")]
    fn init_constructor_rejects_a_long_extended_payload() {
        let payload = [0; CanFrame::MAX_FRAME_LENGTH as usize + 1];
        let _ = CanFrame::from_raw_id(0x345, &payload, true);
    }

    #[test]
    #[should_panic(expected = "CAN frame id")]
    fn init_constructor_rejects_a_large_id() {
        let payload = [0; CanFrame::MAX_FRAME_LENGTH as usize];
        let _ = CanFrame::from_raw_id(0xffff_ffff, &payload, true);
    }

    #[test]
    fn equality() {
        let mut frame1 = CanFrame::new();
        let mut frame2 = CanFrame::new();
        assert!(frame1 == frame2);
        frame1.set_id(0x1);
        assert!(frame1 != frame2);
        frame1.set_id(0x100);
        frame2.set_id(0x100);
        let p1 = [0; 8];
        let p2 = [0; 8];
        for i in 1..=CanFrame::MAX_FRAME_LENGTH {
            frame1.set_payload(&p1[..usize::from(i)]);
            frame2.set_payload(&p2[..usize::from(i)]);
            for j in 0..i {
                frame1.payload_mut()[usize::from(j)] = j;
                frame2.payload_mut()[usize::from(j)] = j;
            }
            assert!(frame1 == frame2);
        }
        let payload = [0, 1, 2, 3, 4, 5, 6, 7];
        frame1.set_id(0x100);
        frame2.set_id(0x100);
        frame1.set_payload(&payload);
        frame2.set_payload(&payload);
        assert!(frame1 == frame2);
        frame1.set_payload_length(4);
        assert!(frame1 != frame2);
        let payload2 = [0, 1, 2, 3, 4, 5, 6, 7];
        frame1.set_payload(&payload);
        frame2.set_payload(&payload2);
        assert!(frame1 == frame2);
        assert_eq!(frame1, frame2);
        frame1.payload_mut()[0] = 1;
        assert!(frame1 != frame2);
        frame1.payload_mut()[0] = 0;
        frame2.set_id(0x17000020);
        assert!(frame1 != frame2);
        frame2.set_id(0x100);
        assert!(frame1 == frame2);
        // Lengths over the maximum are ignored.
        frame1.set_payload_length(4);
        frame1.set_payload_length(CanFrame::MAX_FRAME_LENGTH);
        frame1.set_payload_length(4);
        frame1.set_payload_length(CanFrame::MAX_FRAME_LENGTH + 1);
        assert_eq!(frame1.payload_length(), 4);
    }

    #[test]
    fn setter() {
        let mut frame = CanFrame::new();
        let payload: [u8; 8] = core::array::from_fn(|i| i as u8);
        frame.set_id(0x555);
        assert_eq!(frame.id(), 0x555);
        let ext_id = can_id::extended(0x17002345);
        frame.set_id(ext_id);
        assert_eq!(frame.id(), ext_id);
        assert_eq!(CanFrame::max_payload_length(), CanFrame::MAX_FRAME_LENGTH);
        frame.set_payload(&payload);
        assert_eq!(frame.payload_length(), CanFrame::MAX_FRAME_LENGTH);
        for i in 0..=CanFrame::MAX_FRAME_LENGTH {
            frame.set_payload(&payload[..usize::from(i)]);
            assert_eq!(frame.payload_length(), i);
        }
        for i in 0..=CanFrame::MAX_FRAME_LENGTH {
            frame.set_payload_length(i);
            assert_eq!(frame.payload_length(), i);
        }
        frame.set_timestamp(0x23428242);
        assert_eq!(frame.timestamp(), 0x23428242);
    }

    #[test]
    #[should_panic(expected = "CAN frame length")]
    fn setter_rejects_a_long_payload() {
        let mut frame = CanFrame::new();
        frame.set_payload(&[0; CanFrame::MAX_FRAME_LENGTH as usize + 1]);
    }

    #[test]
    fn assignment() {
        let frame1 = CanFrame::from_raw_id(0x0, &[0x00, 0x01, 0x02, 0x03], false);
        let mut frame2 = CanFrame::from_raw_id(0x0, &[0; 8], false);
        assert!(frame1 != frame2);
        frame2 = frame1;
        assert!(frame1 == frame2);
        assert_eq!(frame1.id(), frame2.id());
        assert_eq!(frame1.payload_length(), frame2.payload_length());
        assert_eq!(frame1.payload(), frame2.payload());
    }

    #[test]
    fn with_id_fills_the_payload() {
        let frame = CanFrame::with_id(0x555);
        assert_eq!(frame.id(), 0x555);
        assert_eq!(frame.payload_length(), 0);
        assert_eq!(*frame.payload_mut_for_test(), [0xFF; 8]);
    }

    impl CanFrame {
        fn payload_mut_for_test(&self) -> &[u8; 8] {
            &self.payload
        }
    }
}
