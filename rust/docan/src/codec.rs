// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The frame codec, ported from `datalink/DoCanFrameCodecConfig.h`,
//! `DoCanFrameCodecConfigPresets.h`/`.cpp`, `IDoCanFrameSizeMapper.h`,
//! `DoCanDefaultFrameSizeMapper.h`, `DoCanFdFrameSizeMapper.h` and `DoCanFrameCodec.h`.

use crate::common::{FlowStatus, FrameIndex, FrameSize, FrameType, MessageSize};

/// The smallest and largest size of one frame kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SizeConfig {
    /// The smallest size; shorter frames are padded.
    pub min: FrameSize,
    /// The largest size.
    pub max: FrameSize,
}

/// How frames are laid out: the port of `DoCanFrameCodecConfig<uint8_t>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameCodecConfig {
    /// Single frames.
    pub single_frame_size: SizeConfig,
    /// First frames.
    pub first_frame_size: SizeConfig,
    /// Consecutive frames.
    pub consecutive_frame_size: SizeConfig,
    /// Flow control frames.
    pub flow_control_frame_size: SizeConfig,
    /// The padding byte.
    pub filler: u8,
    /// Bytes before the protocol control information (1 for extended addressing).
    pub offset: u8,
}

/// The `DoCanFrameCodecConfigPresets`.
pub mod presets {
    use super::{FrameCodecConfig, SizeConfig};

    const fn config(
        sf: (u8, u8),
        ff: (u8, u8),
        cf: (u8, u8),
        fc: (u8, u8),
        offset: u8,
    ) -> FrameCodecConfig {
        FrameCodecConfig {
            single_frame_size: SizeConfig { min: sf.0, max: sf.1 },
            first_frame_size: SizeConfig { min: ff.0, max: ff.1 },
            consecutive_frame_size: SizeConfig { min: cf.0, max: cf.1 },
            flow_control_frame_size: SizeConfig { min: fc.0, max: fc.1 },
            filler: 0xCC,
            offset,
        }
    }

    /// Classic CAN, every frame padded to 8 bytes.
    pub const PADDED_CLASSIC: FrameCodecConfig = config((8, 8), (8, 8), (8, 8), (8, 8), 0);
    /// CAN FD, padded.
    pub const PADDED_FD: FrameCodecConfig = config((8, 64), (64, 64), (8, 64), (8, 64), 0);
    /// Classic CAN, frames as short as their content.
    pub const OPTIMIZED_CLASSIC: FrameCodecConfig = config((0, 8), (8, 8), (0, 8), (0, 8), 0);
    /// CAN FD, frames as short as their content.
    pub const OPTIMIZED_FD: FrameCodecConfig = config((0, 64), (64, 64), (0, 64), (0, 64), 0);
    /// Extended addressing (one address byte first), classic, padded.
    pub const EA_PADDED_CLASSIC: FrameCodecConfig = config((8, 8), (8, 8), (8, 8), (8, 8), 1);
    /// Extended addressing, CAN FD, padded.
    pub const EA_PADDED_FD: FrameCodecConfig = config((8, 64), (64, 64), (8, 64), (8, 64), 1);
    /// Extended addressing, classic, optimized.
    pub const EA_OPTIMIZED_CLASSIC: FrameCodecConfig = config((0, 8), (8, 8), (0, 8), (0, 8), 1);
    /// Extended addressing, CAN FD, optimized.
    pub const EA_OPTIMIZED_FD: FrameCodecConfig = config((0, 64), (64, 64), (0, 64), (0, 64), 1);
}

/// Rounds a frame size up to one the bus can carry: the port of `IDoCanFrameSizeMapper`.
pub trait FrameSizeMapper: Sync {
    /// The size to send a frame of `size` bytes with, or `None` if none fits.
    fn map_frame_size(&self, size: FrameSize) -> Option<FrameSize>;
}

/// Every size is fine (`DoCanDefaultFrameSizeMapper`).
pub struct DefaultFrameSizeMapper;

impl FrameSizeMapper for DefaultFrameSizeMapper {
    fn map_frame_size(&self, size: FrameSize) -> Option<FrameSize> {
        Some(size)
    }
}

/// The CAN FD data length codes (`DoCanFdFrameSizeMapper`).
pub struct FdFrameSizeMapper;

impl FrameSizeMapper for FdFrameSizeMapper {
    fn map_frame_size(&self, size: FrameSize) -> Option<FrameSize> {
        const SIZES: [u8; 65] = [
            0, 1, 2, 3, 4, 5, 6, 7, 8, 12, 12, 12, 12, 16, 16, 16, 16, 20, 20, 20, 20, 24, 24, 24,
            24, 32, 32, 32, 32, 32, 32, 32, 32, 48, 48, 48, 48, 48, 48, 48, 48, 48, 48, 48, 48, 48,
            48, 48, 48, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64,
        ];
        SIZES.get(usize::from(size)).copied()
    }
}

/// Why a frame could not be decoded or encoded (`CodecResult` without `OK`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodecError {
    /// The frame has a size the config does not allow.
    InvalidFrameSize,
    /// The message size does not fit the frame kind.
    InvalidMessageSize,
    /// No frame of that index exists for the data.
    InvalidFrameIndex,
    /// Not one of the four frame kinds.
    InvalidFrameType,
}

/// The payload size up to which the single frame data length is one nibble.
pub const EXTENDED_SF_DL_EDGE_SIZE: u8 = 8;
/// The message size above which the first frame carries a 32-bit size.
pub const ESCAPED_SEQ_MESSAGE_SIZE: u16 = 4095;

/// A decoded first frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirstFrame<'a> {
    /// The whole message's size.
    pub message_size: MessageSize,
    /// How many frames the message takes.
    pub frame_count: FrameIndex,
    /// How many data bytes each consecutive frame carries.
    pub consecutive_frame_data_size: FrameSize,
    /// The data in this frame.
    pub data: &'a [u8],
}

/// Cuts messages into frames and reads them back, under one [`FrameCodecConfig`].
pub struct FrameCodec {
    config: &'static FrameCodecConfig,
    mapper: &'static dyn FrameSizeMapper,
}

impl FrameCodec {
    /// A codec.
    pub const fn new(
        config: &'static FrameCodecConfig,
        mapper: &'static dyn FrameSizeMapper,
    ) -> Self {
        Self { config, mapper }
    }

    /// The config.
    pub const fn config(&self) -> &'static FrameCodecConfig {
        self.config
    }

    /// The kind of the frame in `payload`.
    pub fn decode_frame_type(&self, payload: &[u8]) -> Result<FrameType, CodecError> {
        let offset = usize::from(self.config.offset);
        if payload.len() <= offset {
            return Err(CodecError::InvalidFrameSize);
        }
        FrameType::from_nibble((payload[offset] >> 4) & 0x0F).ok_or(CodecError::InvalidFrameType)
    }

    /// The message in a single frame: its size and its data.
    pub fn decode_single_frame<'a>(
        &self,
        payload: &'a [u8],
    ) -> Result<(MessageSize, &'a [u8]), CodecError> {
        let offset = usize::from(self.config.offset);
        if !self.check_frame_size(payload, 2, self.config.single_frame_size) {
            return Err(CodecError::InvalidFrameSize);
        }
        let mut message_size = MessageSize::from(payload[offset] & 0x0F);
        let is_extended_sf_dl_expected = payload.len() > usize::from(EXTENDED_SF_DL_EDGE_SIZE);
        if message_size == 0 && is_extended_sf_dl_expected {
            message_size = MessageSize::from(payload[offset + 1]);
            let check_payload_size = usize::from(message_size) + offset + 2;
            if check_payload_size <= usize::from(self.config.single_frame_size.max)
                && check_payload_size <= payload.len()
            {
                return Ok((
                    message_size,
                    &payload[offset + 2..offset + 2 + usize::from(message_size)],
                ));
            }
        } else if message_size > 0 && !is_extended_sf_dl_expected {
            let check_payload_size = usize::from(message_size) + offset + 1;
            if check_payload_size <= payload.len() {
                return Ok((
                    message_size,
                    &payload[offset + 1..offset + 1 + usize::from(message_size)],
                ));
            }
        }
        Err(CodecError::InvalidMessageSize)
    }

    /// The start of a segmented message in a first frame.
    pub fn decode_first_frame<'a>(&self, payload: &'a [u8]) -> Result<FirstFrame<'a>, CodecError> {
        if !self.check_frame_size(payload, 3, self.config.first_frame_size) {
            return Err(CodecError::InvalidFrameSize);
        }
        let offset = usize::from(self.config.offset);
        let mut message_size = u16::from_be_bytes([payload[offset], payload[offset + 1]]) & 0xFFF;
        let mut data_start = 2usize;
        let consecutive_frame_data_size = (payload.len() - (offset + 1)) as FrameSize;
        if message_size == 0 {
            if !self.check_frame_size(payload, 6, self.config.first_frame_size) {
                return Err(CodecError::InvalidFrameSize);
            }
            let escaped = u32::from_be_bytes([
                payload[offset + 2],
                payload[offset + 3],
                payload[offset + 4],
                payload[offset + 5],
            ]);
            if escaped <= u32::from(ESCAPED_SEQ_MESSAGE_SIZE)
                || escaped > u32::from(MessageSize::MAX)
            {
                return Err(CodecError::InvalidMessageSize);
            }
            message_size = escaped as MessageSize;
            data_start = 6;
        }
        let count = u32::from(message_size) / u32::from(consecutive_frame_data_size.max(1));
        if consecutive_frame_data_size == 0 || count >= u32::from(FrameIndex::MAX) {
            return Err(CodecError::InvalidMessageSize);
        }
        let frame_count = (count + 1) as FrameIndex;
        if frame_count > 1
            && !self.fits_into_short_single_frame(message_size)
            && !self.fits_into_long_single_frame(message_size)
        {
            return Ok(FirstFrame {
                message_size,
                frame_count,
                consecutive_frame_data_size,
                data: &payload[offset + data_start..],
            });
        }
        Err(CodecError::InvalidMessageSize)
    }

    /// A segment: its sequence number and its data.
    pub fn decode_consecutive_frame<'a>(
        &self,
        payload: &'a [u8],
    ) -> Result<(u8, &'a [u8]), CodecError> {
        if !self.check_frame_size(payload, 2, self.config.consecutive_frame_size) {
            return Err(CodecError::InvalidFrameSize);
        }
        let offset = usize::from(self.config.offset);
        Ok((payload[offset] & 0x0F, &payload[offset + 1..]))
    }

    /// A flow control frame: its status, block size and encoded separation time.
    pub fn decode_flow_control_frame(
        &self,
        payload: &[u8],
    ) -> Result<(FlowStatus, u8, u8), CodecError> {
        if !self.check_frame_size(payload, 3, self.config.flow_control_frame_size) {
            return Err(CodecError::InvalidFrameSize);
        }
        let offset = usize::from(self.config.offset);
        Ok((
            FlowStatus::from_nibble(payload[offset] & 0x0F),
            payload[offset + 1],
            payload[offset + 2],
        ))
    }

    /// How many frames a message of `message_size` takes, and the data per consecutive
    /// frame (0 for a single frame).
    ///
    /// # Panics
    ///
    /// If the config's consecutive frames cannot hold two bytes past the offset (the C++
    /// asserts).
    pub fn encoded_frame_count(
        &self,
        message_size: MessageSize,
    ) -> Result<(FrameIndex, FrameSize), CodecError> {
        assert!(
            u16::from(self.config.consecutive_frame_size.max) >= u16::from(self.config.offset) + 2,
            "consecutive frame size must be large enough"
        );
        if message_size == 0 {
            return Err(CodecError::InvalidMessageSize);
        }
        if self.fits_into_short_single_frame(message_size)
            || self.fits_into_long_single_frame(message_size)
        {
            return Ok((1, 0));
        }
        let consecutive_frame_data_size =
            self.config.consecutive_frame_size.max - (self.config.offset + 1);
        let count = u32::from(message_size) / u32::from(consecutive_frame_data_size);
        if count >= u32::from(FrameIndex::MAX) {
            return Err(CodecError::InvalidMessageSize);
        }
        Ok(((count + 1) as FrameIndex, consecutive_frame_data_size))
    }

    /// Write frame `frame_index` of `data` into `payload`; returns the frame's padded size
    /// and how many data bytes it consumed.
    pub fn encode_data_frame(
        &self,
        payload: &mut [u8],
        data: &[u8],
        frame_index: FrameIndex,
        consecutive_frame_data_size: FrameSize,
    ) -> Result<(usize, FrameSize), CodecError> {
        if data.len() > usize::from(MessageSize::MAX) {
            return Err(CodecError::InvalidFrameIndex);
        }
        self.encode_data_frame_head(
            payload,
            data.len() as MessageSize,
            data,
            frame_index,
            consecutive_frame_data_size,
        )
    }

    /// As [`encode_data_frame`](Self::encode_data_frame) for the `pending_message_size`
    /// bytes still to send, of which `head` holds at least the ones this frame takes.
    pub fn encode_data_frame_head(
        &self,
        payload: &mut [u8],
        pending_message_size: MessageSize,
        head: &[u8],
        frame_index: FrameIndex,
        consecutive_frame_data_size: FrameSize,
    ) -> Result<(usize, FrameSize), CodecError> {
        if pending_message_size == 0 {
            return Err(CodecError::InvalidFrameIndex);
        }
        let data = head;
        let offset = usize::from(self.config.offset);
        let dest_offset: usize;
        let consumed_data_size: FrameSize;
        let min_frame_size: FrameSize;
        if frame_index == 0 {
            if self.fits_into_short_single_frame(pending_message_size) {
                payload[offset] =
                    ((FrameType::Single as u8) << 4) | (pending_message_size as u8 & 0x0F);
                dest_offset = offset + 1;
                consumed_data_size = pending_message_size as FrameSize;
                min_frame_size = self.config.single_frame_size.min;
            } else if self.fits_into_long_single_frame(pending_message_size) {
                payload[offset] = (FrameType::Single as u8) << 4;
                payload[offset + 1] = pending_message_size as u8;
                dest_offset = offset + 2;
                consumed_data_size = pending_message_size as FrameSize;
                min_frame_size = self.config.single_frame_size.min;
            } else if consecutive_frame_data_size > 0 {
                let mut dest = offset + 2;
                let mut consumed = consecutive_frame_data_size.wrapping_sub(1);
                if pending_message_size <= ESCAPED_SEQ_MESSAGE_SIZE {
                    let bytes = (pending_message_size & 0xFFF).to_be_bytes();
                    payload[offset] = bytes[0];
                    payload[offset + 1] = bytes[1];
                } else {
                    payload[offset] = 0;
                    payload[offset + 1] = 0;
                    payload[offset + 2..offset + 6]
                        .copy_from_slice(&u32::from(pending_message_size).to_be_bytes());
                    dest += 4;
                    consumed = consumed.wrapping_sub(4);
                }
                payload[offset] |= (FrameType::First as u8) << 4;
                dest_offset = dest;
                consumed_data_size = consumed;
                min_frame_size = self.config.first_frame_size.min;
            } else {
                return Err(CodecError::InvalidFrameIndex);
            }
        } else if consecutive_frame_data_size > 0 {
            payload[offset] = ((FrameType::Consecutive as u8) << 4) | (frame_index as u8 & 0x0F);
            dest_offset = offset + 1;
            consumed_data_size = (u32::from(consecutive_frame_data_size)
                .min(u32::from(pending_message_size)))
                as FrameSize;
            min_frame_size = consumed_data_size.max(self.config.consecutive_frame_size.min);
        } else {
            return Err(CodecError::InvalidFrameIndex);
        }
        let payload_size = dest_offset + usize::from(consumed_data_size);
        if payload_size > payload.len() || usize::from(consumed_data_size) > data.len() {
            return Err(CodecError::InvalidFrameSize);
        }
        payload[dest_offset..payload_size]
            .copy_from_slice(&data[..usize::from(consumed_data_size)]);
        let padded = self.adjust_frame(payload, payload_size, min_frame_size)?;
        Ok((padded, consumed_data_size))
    }

    /// Write a flow control frame into `payload`; returns the frame's padded size.
    pub fn encode_flow_control_frame(
        &self,
        payload: &mut [u8],
        flow_status: FlowStatus,
        block_size: u8,
        encoded_min_separation_time: u8,
    ) -> Result<usize, CodecError> {
        let offset = usize::from(self.config.offset);
        if payload.len() < offset + 3 {
            return Err(CodecError::InvalidFrameSize);
        }
        payload[offset] = ((FrameType::FlowControl as u8) << 4) | flow_status.nibble();
        payload[offset + 1] = block_size;
        payload[offset + 2] = encoded_min_separation_time;
        self.adjust_frame(payload, offset + 3, self.config.flow_control_frame_size.min)
    }

    fn adjust_frame(
        &self,
        payload: &mut [u8],
        payload_size: usize,
        min_frame_size: FrameSize,
    ) -> Result<usize, CodecError> {
        let padded = self
            .mapper
            .map_frame_size(payload_size.max(usize::from(min_frame_size)) as FrameSize)
            .ok_or(CodecError::InvalidFrameSize)?;
        let padded = usize::from(padded);
        if padded > payload_size {
            if padded > payload.len() {
                return Err(CodecError::InvalidFrameSize);
            }
            payload[payload_size..padded].fill(self.config.filler);
        }
        Ok(padded)
    }

    fn check_frame_size(
        &self,
        payload: &[u8],
        min_payload_size: FrameSize,
        frame_size: SizeConfig,
    ) -> bool {
        payload.len() >= usize::from(self.config.offset) + usize::from(min_payload_size)
            && payload.len() >= usize::from(frame_size.min)
            && payload.len() <= usize::from(frame_size.max)
    }

    fn fits_into_short_single_frame(&self, message_size: MessageSize) -> bool {
        let edge = EXTENDED_SF_DL_EDGE_SIZE.min(self.config.single_frame_size.max);
        let offset = u16::from(self.config.offset) + 1;
        message_size <= u16::from(edge).wrapping_sub(offset)
    }

    fn fits_into_long_single_frame(&self, message_size: MessageSize) -> bool {
        assert!(
            u16::from(self.config.single_frame_size.max) >= u16::from(self.config.offset) + 2,
            "single frame size must be large enough"
        );
        let offset = u16::from(self.config.offset) + 2;
        message_size <= u16::from(self.config.single_frame_size.max) - offset
    }
}

// Ported from docan/test/src/docan/datalink/DoCanFrameCodecTest.cpp, with message sizes
// in the demo's 16-bit range.
#[cfg(test)]
mod tests {
    use super::*;

    static FD_MAPPER: FdFrameSizeMapper = FdFrameSizeMapper;

    const fn config(
        sf: (u8, u8),
        ff: (u8, u8),
        cf: (u8, u8),
        fc: (u8, u8),
        filler: u8,
        offset: u8,
    ) -> FrameCodecConfig {
        FrameCodecConfig {
            single_frame_size: SizeConfig { min: sf.0, max: sf.1 },
            first_frame_size: SizeConfig { min: ff.0, max: ff.1 },
            consecutive_frame_size: SizeConfig { min: cf.0, max: cf.1 },
            flow_control_frame_size: SizeConfig { min: fc.0, max: fc.1 },
            filler,
            offset,
        }
    }

    #[test]
    fn decode_first_frames_with_escape_seq_and_short_min_frame() {
        static SHORT_FF_CLASSIC: FrameCodecConfig = config((0, 8), (6, 8), (0, 8), (0, 8), 0xCC, 1);
        let cut = FrameCodec::new(&SHORT_FF_CLASSIC, &FD_MAPPER);
        let escape_seq_payload = [0xab, 0x10, 0x00, 0x12, 0x34, 0x56];
        let normal_payload = [0xab, 0x11, 0x23, 0xAB, 0xCD, 0xEF];
        assert_eq!(cut.decode_first_frame(&escape_seq_payload), Err(CodecError::InvalidFrameSize));
        let first = cut.decode_first_frame(&normal_payload).unwrap();
        assert_eq!(first.message_size, 0x123);
        assert_eq!(first.frame_count, 73);
        assert_eq!(first.consecutive_frame_data_size, 4);
        assert_eq!(first.data, &normal_payload[3..]);
        // An escaped size within 16 bits.
        let payload = [0xab, 0x10, 0x00, 0x00, 0x00, 0x12, 0x34, 0xA5];
        let first = cut.decode_first_frame(&payload).unwrap();
        assert_eq!(first.message_size, 0x1234);
        assert_eq!(first.frame_count, 0x1234 / 6 + 1);
        assert_eq!(first.consecutive_frame_data_size, 6);
        assert_eq!(first.data, &payload[7..]);
        // An escaped size that fits 12 bits, or exceeds the message size type, is refused.
        assert_eq!(
            cut.decode_first_frame(&[0xab, 0x10, 0x00, 0x00, 0x00, 0x0F, 0xFF]),
            Err(CodecError::InvalidMessageSize)
        );
        assert_eq!(
            cut.decode_first_frame(&[0xab, 0x10, 0x00, 0x00, 0x01, 0x00, 0x00]),
            Err(CodecError::InvalidMessageSize)
        );
        assert_eq!(
            cut.decode_first_frame(&[0xab, 0x10, 0x00, 0xFF, 0xFF, 0xFF, 0xFF]),
            Err(CodecError::InvalidMessageSize)
        );
        assert_eq!(cut.encoded_frame_count(6 * 100 - 1), Ok((100, 6)));
        assert_eq!(cut.encoded_frame_count(MessageSize::MAX), Ok((MessageSize::MAX / 6 + 1, 6)));
    }

    #[test]
    fn first_frame_fits_into_long_single_frame() {
        static CUSTOM_FD: FrameCodecConfig = config((0, 64), (0, 64), (0, 64), (0, 64), 0xCC, 1);
        let cut = FrameCodec::new(&CUSTOM_FD, &FD_MAPPER);
        let payload = [0xAB, 0x10, 0x0B, 0x01, 0x23, 0x45, 0x67, 0x89, 0xAB];
        assert_eq!(cut.decode_first_frame(&payload), Err(CodecError::InvalidMessageSize));
    }

    #[test]
    #[should_panic(expected = "single frame size")]
    fn single_frame_max_size_smaller_than_offset() {
        static CUSTOM_FD: FrameCodecConfig = config((0, 2), (0, 64), (0, 64), (0, 64), 0xCC, 1);
        let cut = FrameCodec::new(&CUSTOM_FD, &FD_MAPPER);
        let payload = [0xAB, 0x10, 0x0B, 0x01, 0x23, 0x45, 0x67, 0x89, 0xAB];
        let _ = cut.decode_first_frame(&payload);
    }

    #[test]
    #[should_panic(expected = "consecutive frame size")]
    fn consecutive_frame_max_size_smaller_than_offset() {
        static CUSTOM_FD: FrameCodecConfig = config((0, 64), (0, 64), (0, 2), (0, 64), 0xCC, 1);
        let cut = FrameCodec::new(&CUSTOM_FD, &FD_MAPPER);
        let _ = cut.encoded_frame_count(0);
    }

    #[test]
    fn decode_flow_control_frames_with_full_size() {
        static CONFIG: FrameCodecConfig = config((8, 8), (8, 8), (8, 8), (8, 8), 0xCC, 1);
        let cut = FrameCodec::new(&CONFIG, &FD_MAPPER);
        assert_eq!(
            cut.decode_flow_control_frame(&[0xab, 0x32, 0x12, 0x34, 0xaa, 0xaa, 0xaa, 0xaa]),
            Ok((FlowStatus::Ovflw, 0x12, 0x34))
        );
        assert_eq!(
            cut.decode_flow_control_frame(&[0xab, 0x34, 0x12, 0x34]),
            Err(CodecError::InvalidFrameSize)
        );
        assert_eq!(
            cut.decode_flow_control_frame(&[0xab, 0x32, 0x12, 0x34, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa]),
            Err(CodecError::InvalidFrameSize)
        );
    }

    #[test]
    fn decode_frames_with_enforced_padding() {
        static CONFIG: FrameCodecConfig = config((6, 6), (6, 6), (6, 6), (6, 6), 0xCC, 1);
        let cut = FrameCodec::new(&CONFIG, &FD_MAPPER);
        let payload = [0xab, 0x01, 0x12, 0xaa, 0xaa, 0xaa];
        assert_eq!(cut.decode_single_frame(&payload), Ok((1, &payload[2..3])));
        let payload = [0xab, 0x04, 0x12, 0x34, 0x56, 0x78];
        assert_eq!(cut.decode_single_frame(&payload), Ok((4, &payload[2..])));
        assert_eq!(
            cut.decode_single_frame(&[0xab, 0x06, 0x12, 0x34, 0x56]),
            Err(CodecError::InvalidFrameSize)
        );
        assert_eq!(
            cut.decode_single_frame(&[0xab, 0x06, 0x12, 0x34, 0x56, 0x78, 0x9a]),
            Err(CodecError::InvalidFrameSize)
        );
        let payload = [0xab, 0x10, 0x05, 0x12, 0x34, 0x56];
        let first = cut.decode_first_frame(&payload).unwrap();
        assert_eq!(
            (first.message_size, first.frame_count, first.consecutive_frame_data_size),
            (5, 2, 4)
        );
        assert_eq!(first.data, &payload[3..]);
        assert_eq!(
            cut.decode_single_frame(&[0xab, 0x10, 0x07, 0x12, 0x34]),
            Err(CodecError::InvalidFrameSize)
        );
        let payload = [0xab, 0x23, 0x12, 0xaa, 0xaa, 0xaa];
        assert_eq!(cut.decode_consecutive_frame(&payload), Ok((3, &payload[2..])));
        assert_eq!(
            cut.decode_consecutive_frame(&[0xab, 0x23, 0x12, 0xaa, 0xaa]),
            Err(CodecError::InvalidFrameSize)
        );
        assert_eq!(
            cut.decode_consecutive_frame(&[0xab, 0x23, 0x12, 0xaa, 0xaa, 0xaa, 0xaa]),
            Err(CodecError::InvalidFrameSize)
        );
        assert_eq!(
            cut.decode_flow_control_frame(&[0xab, 0x32, 0x12, 0x34, 0xaa, 0xaa]),
            Ok((FlowStatus::Ovflw, 0x12, 0x34))
        );
        assert_eq!(
            cut.decode_flow_control_frame(&[0xab, 0x32, 0x12, 0x34, 0xaa]),
            Err(CodecError::InvalidFrameSize)
        );
        assert_eq!(
            cut.decode_flow_control_frame(&[0xab, 0x32, 0x12, 0x34, 0xaa, 0xaa, 0xaa]),
            Err(CodecError::InvalidFrameSize)
        );
    }

    #[test]
    fn decode_short_and_long_frames_with_enforced_padding_unmapped() {
        static CONFIG: FrameCodecConfig = config((6, 14), (14, 14), (6, 14), (6, 14), 0xCC, 1);
        let cut = FrameCodec::new(&CONFIG, &FD_MAPPER);
        let payload = [0xab, 0x01, 0x12, 0xaa, 0xaa, 0xaa];
        assert_eq!(cut.decode_single_frame(&payload), Ok((1, &payload[2..3])));
        let payload = [0xab, 0x04, 0x12, 0x34, 0x56, 0x78];
        assert_eq!(cut.decode_single_frame(&payload), Ok((4, &payload[2..])));
        let mut payload = [0u8; 14];
        payload[..8].copy_from_slice(&[0xab, 0x00, 0x05, 0x12, 0xaa, 0xaa, 0xaa, 0xaa]);
        assert_eq!(cut.decode_single_frame(&payload), Ok((5, &payload[3..8])));
        let mut payload = [0u8; 14];
        payload[..8].copy_from_slice(&[0xab, 0x00, 0x0b, 0x04, 0x12, 0x34, 0x56, 0x78]);
        assert_eq!(cut.decode_single_frame(&payload), Ok((11, &payload[3..])));
        assert_eq!(
            cut.decode_single_frame(&[0xab, 0x06, 0x12, 0x34, 0x56]),
            Err(CodecError::InvalidFrameSize)
        );
        assert_eq!(
            cut.decode_single_frame(&[0xab, 0x06, 0x12, 0x34, 0x56, 0x78, 0x9a]),
            Err(CodecError::InvalidMessageSize)
        );
        let mut payload = [0u8; 13];
        payload[..5].copy_from_slice(&[0xab, 0x06, 0x12, 0x34, 0x56]);
        assert_eq!(cut.decode_single_frame(&payload), Err(CodecError::InvalidMessageSize));
        let mut payload = [0u8; 15];
        payload[..7].copy_from_slice(&[0xab, 0x06, 0x12, 0x34, 0x56, 0x78, 0x9a]);
        assert_eq!(cut.decode_single_frame(&payload), Err(CodecError::InvalidFrameSize));
        let mut payload = [0u8; 14];
        payload[..6].copy_from_slice(&[0xab, 0x10, 12, 0x12, 0x34, 0x56]);
        let first = cut.decode_first_frame(&payload).unwrap();
        assert_eq!(
            (first.message_size, first.frame_count, first.consecutive_frame_data_size),
            (12, 2, 12)
        );
        assert_eq!(first.data, &payload[3..]);
        payload[2] = 0x0d;
        let first = cut.decode_first_frame(&payload).unwrap();
        assert_eq!(
            (first.message_size, first.frame_count, first.consecutive_frame_data_size),
            (13, 2, 12)
        );
        assert_eq!(
            cut.decode_first_frame(&[0xab, 0x10, 0x07, 0x12, 0x34]),
            Err(CodecError::InvalidFrameSize)
        );
        assert_eq!(
            cut.decode_first_frame(&[0xab, 0x10, 0x07, 0x12, 0x34, 0x56, 0x78]),
            Err(CodecError::InvalidFrameSize)
        );
        let payload = [0xab, 0x23, 0x12, 0xaa, 0xaa, 0xaa];
        assert_eq!(cut.decode_consecutive_frame(&payload), Ok((3, &payload[2..])));
        let mut payload = [0u8; 14];
        payload[..6].copy_from_slice(&[0xab, 0x23, 0x12, 0xaa, 0xaa, 0xaa]);
        assert_eq!(cut.decode_consecutive_frame(&payload), Ok((3, &payload[2..])));
        assert_eq!(
            cut.decode_consecutive_frame(&[0xab, 0x23, 0x12, 0xaa, 0xaa]),
            Err(CodecError::InvalidFrameSize)
        );
        assert_eq!(
            cut.decode_flow_control_frame(&[0xab, 0x32, 0x12, 0x34, 0xaa, 0xaa]),
            Ok((FlowStatus::Ovflw, 0x12, 0x34))
        );
        assert_eq!(
            cut.decode_flow_control_frame(&[0xab, 0x32, 0x12, 0x34, 0xaa]),
            Err(CodecError::InvalidFrameSize)
        );
    }

    #[test]
    fn encode_data_frame() {
        static CONFIG: FrameCodecConfig = config((3, 8), (8, 8), (3, 8), (3, 8), 0xCC, 1);
        let cut = FrameCodec::new(&CONFIG, &FD_MAPPER);
        let mut frame = [0u8; 10];
        frame[0] = 0xdc;
        frame[9] = 0xef;
        let (len, consumed) = cut.encode_data_frame(&mut frame, &[0x12, 0x34, 0x78], 0, 0).unwrap();
        assert_eq!(&frame[..len], &[0xdc, 0x03, 0x12, 0x34, 0x78]);
        assert_eq!(consumed, 3);
        let message = [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde];
        let (len, consumed) = cut.encode_data_frame(&mut frame, &message, 0, 6).unwrap();
        assert_eq!(&frame[..len], &[0xdc, 0x10, 0x07, 0x12, 0x34, 0x56, 0x78, 0x9a]);
        assert_eq!(consumed, 5);
        let (len, consumed) = cut.encode_data_frame(&mut frame, &message[5..], 1, 6).unwrap();
        assert_eq!(&frame[..len], &[0xdc, 0x21, 0xbc, 0xde]);
        assert_eq!(consumed, 2);
        // One data byte per consecutive frame: the first frame carries none.
        let (len, consumed) = cut.encode_data_frame(&mut frame, &message, 0, 1).unwrap();
        assert_eq!(&frame[..len], &[0xdc, 0x10, 0x07, 0xcc, 0xcc, 0xcc, 0xcc, 0xcc]);
        assert_eq!(consumed, 0);
        for (index, byte) in message.iter().enumerate() {
            let (len, consumed) = cut
                .encode_data_frame(&mut frame, &message[index..], index as FrameIndex + 1, 1)
                .unwrap();
            assert_eq!(&frame[..len], &[0xdc, 0x20 + index as u8 + 1, *byte]);
            assert_eq!(consumed, 1);
        }
        // A long message takes the escape sequence; sequence numbers wrap.
        const MESSAGE_SIZE: usize = 4321;
        const CF_SIZE: FrameSize = 6;
        let big: [u8; MESSAGE_SIZE] = core::array::from_fn(|i| (i + 0xA5) as u8);
        let (len, consumed) = cut.encode_data_frame(&mut frame, &big, 0, CF_SIZE).unwrap();
        assert_eq!(&frame[..len], &[0xdc, 0x10, 0x00, 0x00, 0x00, 0x10, 0xE1, big[0]]);
        assert_eq!(consumed, 1);
        let mut data = &big[1..];
        for i in 0..(MESSAGE_SIZE - 1) / 6 {
            let (len, consumed) =
                cut.encode_data_frame(&mut frame, data, i as FrameIndex + 1, CF_SIZE).unwrap();
            assert_eq!(frame[1], 0x20 + ((1 + i) % 0x10) as u8);
            assert_eq!(&frame[2..len], &data[..6]);
            assert_eq!(consumed, 6);
            data = &data[6..];
        }
        assert!(data.is_empty());
        // Nothing to send, or no consecutive frame size where one is needed.
        assert_eq!(
            cut.encode_data_frame(&mut frame, &message, 0, 0),
            Err(CodecError::InvalidFrameIndex)
        );
        assert_eq!(
            cut.encode_data_frame(&mut frame[..4], &[0x12, 0x34, 0x78], 0, 0),
            Err(CodecError::InvalidFrameSize)
        );
        assert_eq!(
            cut.encode_data_frame(&mut frame, &[], 0, 0),
            Err(CodecError::InvalidFrameIndex)
        );
        assert_eq!(
            cut.encode_data_frame(&mut frame, &message, 1, 0),
            Err(CodecError::InvalidFrameIndex)
        );
        assert_eq!(frame[9], 0xef);
    }

    #[test]
    fn codec_config_minimum_larger_than_max_can_frame_size() {
        static CUSTOM_FD: FrameCodecConfig = config((0, 64), (0, 64), (0, 64), (65, 65), 0xCC, 1);
        let cut = FrameCodec::new(&CUSTOM_FD, &FD_MAPPER);
        let mut frame = [0u8; 10];
        assert_eq!(
            cut.encode_flow_control_frame(&mut frame, FlowStatus::Cts, 0x13, 0x25),
            Err(CodecError::InvalidFrameSize)
        );
    }

    #[test]
    fn encode_frames_with_enforced_padding() {
        static CONFIG: FrameCodecConfig = config((8, 8), (8, 8), (8, 8), (8, 8), 0x91, 1);
        let cut = FrameCodec::new(&CONFIG, &FD_MAPPER);
        let mut frame = [0u8; 9];
        frame[0] = 0xdc;
        frame[8] = 0xef;
        let (len, consumed) = cut.encode_data_frame(&mut frame, &[0x12, 0x34, 0x78], 0, 0).unwrap();
        assert_eq!(&frame[..len], &[0xdc, 0x03, 0x12, 0x34, 0x78, 0x91, 0x91, 0x91]);
        assert_eq!(consumed, 3);
        let message = [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde];
        let (len, consumed) = cut.encode_data_frame(&mut frame, &message, 0, 6).unwrap();
        assert_eq!(&frame[..len], &[0xdc, 0x10, 0x07, 0x12, 0x34, 0x56, 0x78, 0x9a]);
        assert_eq!(consumed, 5);
        let (len, consumed) = cut.encode_data_frame(&mut frame, &message[5..], 1, 6).unwrap();
        assert_eq!(&frame[..len], &[0xdc, 0x21, 0xbc, 0xde, 0x91, 0x91, 0x91, 0x91]);
        assert_eq!(consumed, 2);
        let mut longer = [0u8; 12];
        longer[0] = 0xdc;
        let (len, consumed) = cut.encode_data_frame(&mut longer, &message, 0, 7).unwrap();
        assert_eq!(
            &longer[..len],
            &[0xdc, 0x10, 0x07, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0x91, 0x91, 0x91]
        );
        assert_eq!(consumed, 6);
        let len = cut.encode_flow_control_frame(&mut frame, FlowStatus::Cts, 0x13, 0x25).unwrap();
        assert_eq!(&frame[..len], &[0xdc, 0x30, 0x13, 0x25, 0x91, 0x91, 0x91, 0x91]);
        assert_eq!(frame[8], 0xef);
    }

    #[test]
    fn encode_short_and_long_frames_with_enforced_padding() {
        static CONFIG: FrameCodecConfig = config((8, 20), (20, 20), (8, 20), (8, 20), 0x91, 1);
        let cut = FrameCodec::new(&CONFIG, &FD_MAPPER);
        let mut frame = [0u8; 21];
        frame[0] = 0xdc;
        frame[20] = 0xef;
        let (len, consumed) =
            cut.encode_data_frame(&mut frame[..20], &[0x12, 0x34, 0x78], 0, 0).unwrap();
        assert_eq!(&frame[..len], &[0xdc, 0x03, 0x12, 0x34, 0x78, 0x91, 0x91, 0x91]);
        assert_eq!(consumed, 3);
        let data = [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde];
        let (len, consumed) = cut.encode_data_frame(&mut frame[..12], &data, 0, 0).unwrap();
        assert_eq!(
            &frame[..len],
            &[0xdc, 0x00, 0x07, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0x91, 0x91]
        );
        assert_eq!(consumed, 7);
        let message = [
            0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x13, 0x24, 0x35, 0x46, 0x57, 0x68,
            0x79, 0x8a, 0xac, 0x11,
        ];
        let (len, consumed) = cut.encode_data_frame(&mut frame[..20], &message, 0, 16).unwrap();
        assert_eq!(
            &frame[..len],
            &[
                0xdc, 0x10, 18, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x13, 0x24, 0x35,
                0x46, 0x57, 0x68, 0x79, 0x91, 0x91
            ]
        );
        assert_eq!(consumed, 15);
        let (len, consumed) =
            cut.encode_data_frame(&mut frame[..8], &message[15..], 1, 16).unwrap();
        assert_eq!(&frame[..len], &[0xdc, 0x21, 0x8a, 0xac, 0x11, 0x91, 0x91, 0x91]);
        assert_eq!(consumed, 3);
        let len =
            cut.encode_flow_control_frame(&mut frame[..20], FlowStatus::Cts, 0x13, 0x25).unwrap();
        assert_eq!(&frame[..len], &[0xdc, 0x30, 0x13, 0x25, 0x91, 0x91, 0x91, 0x91]);
        assert_eq!(
            cut.encode_data_frame(&mut frame[..7], &[0x12, 0x34, 0x78], 0, 0),
            Err(CodecError::InvalidFrameSize)
        );
        assert_eq!(frame[20], 0xef);
    }

    #[test]
    fn fd_mapper_rounds_up_to_a_dlc() {
        assert_eq!(FdFrameSizeMapper.map_frame_size(9), Some(12));
        assert_eq!(FdFrameSizeMapper.map_frame_size(33), Some(48));
        assert_eq!(FdFrameSizeMapper.map_frame_size(64), Some(64));
        assert_eq!(FdFrameSizeMapper.map_frame_size(65), None);
        assert_eq!(DefaultFrameSizeMapper.map_frame_size(65), Some(65));
    }
}
