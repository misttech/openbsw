// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The types every part of DoCAN shares, ported from `common/DoCanConstants.h`,
//! `DoCanParameters.h`, `DoCanTransportAddressPair.h`, `DoCanJobHandle.h`,
//! `DoCanConnection.h`, `DoCanTimerManagement.h` and `datalink/DoCanDataLinkAddressPair.h`
//! and `DoCanCanDataLinkLayer.h`.

use core::cell::Cell;

use openbsw_cpp2can::can_id;

use crate::codec::FrameCodec;

/// A data link address: the CAN id (`DoCanCanDataLinkLayer::AddressType`).
pub type Address = u32;
/// A message size (`MessageSizeType`).
pub type MessageSize = u16;
/// A frame's size in bytes (`FrameSizeType`).
pub type FrameSize = u8;
/// A frame's index in a message (`FrameIndexType`).
pub type FrameIndex = u16;
/// The address that is none.
pub const INVALID_ADDRESS: Address = can_id::INVALID_ID;

/// The kind of a frame, from its first nibble.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameType {
    /// A whole message.
    Single = 0,
    /// The start of a segmented message.
    First = 1,
    /// A segment.
    Consecutive = 2,
    /// The receiver's flow control.
    FlowControl = 3,
}

impl FrameType {
    /// The frame type of `nibble`, if it is one.
    pub const fn from_nibble(nibble: u8) -> Option<Self> {
        match nibble {
            0 => Some(Self::Single),
            1 => Some(Self::First),
            2 => Some(Self::Consecutive),
            3 => Some(Self::FlowControl),
            _ => None,
        }
    }
}

/// The flow status of a flow control frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlowStatus {
    /// Continue to send.
    Cts,
    /// Wait.
    Wait,
    /// The receiver cannot take the message.
    Ovflw,
    /// Not a flow status (the C++ casts the nibble unchecked).
    Invalid(u8),
}

impl FlowStatus {
    /// The flow status of `nibble`.
    pub const fn from_nibble(nibble: u8) -> Self {
        match nibble {
            0 => Self::Cts,
            1 => Self::Wait,
            2 => Self::Ovflw,
            other => Self::Invalid(other),
        }
    }

    /// The nibble of this flow status.
    pub const fn nibble(self) -> u8 {
        match self {
            Self::Cts => 0,
            Self::Wait => 1,
            Self::Ovflw => 2,
            Self::Invalid(other) => other & 0x0F,
        }
    }
}

/// Identifies a send job: the port of `DoCanJobHandle<uint16_t, uint16_t>`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JobHandle {
    counter: u16,
    user_data: u16,
}

impl JobHandle {
    /// A handle.
    pub const fn new(counter: u16, user_data: u16) -> Self {
        Self { counter, user_data }
    }
}

/// A pair of transport addresses: the port of `DoCanTransportAddressPair`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TransportAddressPair {
    source_id: u16,
    target_id: u16,
}

impl TransportAddressPair {
    /// A pair.
    pub const fn new(source_id: u16, target_id: u16) -> Self {
        Self { source_id, target_id }
    }

    /// Whether the two differ.
    pub const fn is_valid(&self) -> bool {
        self.source_id != self.target_id
    }

    /// The source.
    pub const fn source_id(&self) -> u16 {
        self.source_id
    }

    /// The target.
    pub const fn target_id(&self) -> u16 {
        self.target_id
    }

    /// The pair the other way round.
    pub const fn invert(&self) -> Self {
        Self::new(self.target_id, self.source_id)
    }
}

/// A pair of data link addresses: the port of `DoCanDataLinkAddressPair<uint32_t>`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DataLinkAddressPair {
    reception_address: Address,
    transmission_address: Address,
}

impl DataLinkAddressPair {
    /// A pair.
    pub const fn new(reception_address: Address, transmission_address: Address) -> Self {
        Self { reception_address, transmission_address }
    }

    /// Whether the two differ.
    pub const fn is_valid(&self) -> bool {
        self.reception_address != self.transmission_address
    }

    /// The address frames are received on.
    pub const fn reception_address(&self) -> Address {
        self.reception_address
    }

    /// The address frames are sent to.
    pub const fn transmission_address(&self) -> Address {
        self.transmission_address
    }
}

/// A codec and the two address pairs of one conversation: the port of `DoCanConnection`.
#[derive(Clone, Copy)]
pub struct Connection {
    codec: &'static FrameCodec,
    data_link_address_pair: DataLinkAddressPair,
    transport_address_pair: TransportAddressPair,
}

impl Connection {
    /// A connection.
    pub const fn new(
        codec: &'static FrameCodec,
        data_link_address_pair: DataLinkAddressPair,
        transport_address_pair: TransportAddressPair,
    ) -> Self {
        Self { codec, data_link_address_pair, transport_address_pair }
    }

    /// The codec.
    pub const fn frame_codec(&self) -> &'static FrameCodec {
        self.codec
    }

    /// The data link addresses.
    pub const fn data_link_address_pair(&self) -> &DataLinkAddressPair {
        &self.data_link_address_pair
    }

    /// The transport addresses.
    pub const fn transport_address_pair(&self) -> &TransportAddressPair {
        &self.transport_address_pair
    }
}

impl PartialEq for Connection {
    fn eq(&self, other: &Self) -> bool {
        core::ptr::eq(self.codec, other.codec)
            && self.data_link_address_pair == other.data_link_address_pair
            && self.transport_address_pair == other.transport_address_pair
    }
}

impl core::fmt::Debug for Connection {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Connection")
            .field("data_link", &self.data_link_address_pair)
            .field("transport", &self.transport_address_pair)
            .finish()
    }
}

/// Whether `first_time` is before `second_time` on a wrapping 32-bit clock.
pub const fn time_less(first_time: u32, second_time: u32) -> bool {
    first_time.wrapping_sub(second_time) > i32::MAX as u32
}

/// The timeouts and limits of the protocol: the port of `DoCanParameters`.
pub struct DoCanParameters {
    system_us: &'static (dyn Fn() -> u32 + Sync),
    wait_allocate_timeout: u16,
    wait_rx_timeout: u16,
    wait_tx_callback_timeout: u16,
    wait_flow_control_timeout: u16,
    encoded_min_separation_time: Cell<u8>,
    max_block_size: Cell<u8>,
    max_allocate_retry_count: u8,
    max_flow_control_wait_count: u8,
}

// SAFETY: the two cells are set during configuration; the protocol only reads them.
unsafe impl Sync for DoCanParameters {}

impl DoCanParameters {
    /// Parameters, with the timeouts in milliseconds and the separation time in
    /// microseconds.
    ///
    /// # Panics
    ///
    /// If the separation time is not shorter than each timeout (the C++ asserts).
    #[expect(clippy::too_many_arguments, reason = "the C++ constructor's parameters")]
    pub const fn new(
        system_us: &'static (dyn Fn() -> u32 + Sync),
        wait_allocate_timeout: u16,
        wait_rx_timeout: u16,
        wait_tx_callback_timeout: u16,
        wait_flow_control_timeout: u16,
        max_allocate_retry_count: u8,
        max_flow_control_wait_count: u8,
        min_separation_time_us: u32,
        max_block_size: u8,
    ) -> Self {
        assert!(
            min_separation_time_us < wait_allocate_timeout as u32 * 1000,
            "minimum separation time must be smaller than the allocate timeout"
        );
        assert!(
            min_separation_time_us < wait_rx_timeout as u32 * 1000,
            "minimum separation time must be smaller than the rx timeout"
        );
        assert!(
            min_separation_time_us < wait_tx_callback_timeout as u32 * 1000,
            "minimum separation time must be smaller than the tx timeout"
        );
        assert!(
            min_separation_time_us < wait_flow_control_timeout as u32 * 1000,
            "minimum separation time must be smaller than the flow control timeout"
        );
        Self {
            system_us,
            wait_allocate_timeout,
            wait_rx_timeout,
            wait_tx_callback_timeout,
            wait_flow_control_timeout,
            encoded_min_separation_time: Cell::new(Self::encode_min_separation_time(
                min_separation_time_us,
            )),
            max_block_size: Cell::new(max_block_size),
            max_allocate_retry_count,
            max_flow_control_wait_count,
        }
    }

    /// The time now, in microseconds.
    pub fn now_us(&self) -> u32 {
        (self.system_us)()
    }

    /// How long to wait for a buffer, in milliseconds.
    pub const fn wait_allocate_timeout(&self) -> u16 {
        self.wait_allocate_timeout
    }

    /// How long to wait for the next consecutive frame, in milliseconds.
    pub const fn wait_rx_timeout(&self) -> u16 {
        self.wait_rx_timeout
    }

    /// How long to wait for the driver's transmit callback, in milliseconds.
    pub const fn wait_tx_callback_timeout(&self) -> u16 {
        self.wait_tx_callback_timeout
    }

    /// How long to wait for flow control, in milliseconds.
    pub const fn wait_flow_control_timeout(&self) -> u16 {
        self.wait_flow_control_timeout
    }

    /// How often to retry allocating a buffer.
    pub const fn max_allocate_retry_count(&self) -> u8 {
        self.max_allocate_retry_count
    }

    /// How many `WAIT` flow controls to accept.
    pub const fn max_flow_control_wait_count(&self) -> u8 {
        self.max_flow_control_wait_count
    }

    /// The separation time this side asks for, encoded.
    pub fn encoded_min_separation_time(&self) -> u8 {
        self.encoded_min_separation_time.get()
    }

    /// Set the separation time this side asks for, encoded.
    pub fn set_encoded_min_separation_time(&self, encoded: u8) {
        self.encoded_min_separation_time.set(encoded);
    }

    /// The block size this side asks for.
    pub fn max_block_size(&self) -> u8 {
        self.max_block_size.get()
    }

    /// Set the block size this side asks for.
    pub fn set_max_block_size(&self, max_block_size: u8) {
        self.max_block_size.set(max_block_size);
    }

    /// The microseconds an encoded separation time stands for.
    pub const fn decode_min_separation_time(encoded: u8) -> u32 {
        if encoded <= 0x7F {
            return encoded as u32 * 1000;
        }
        if encoded >= 0xF1 && encoded <= 0xF9 {
            return (encoded as u32 - 0xF0) * 100;
        }
        0x7F * 1000
    }

    /// The encoding of a separation time in microseconds.
    pub const fn encode_min_separation_time(min_separation_time_us: u32) -> u8 {
        if min_separation_time_us >= 100 && min_separation_time_us < 1000 {
            return (min_separation_time_us / 100) as u8 + 0xF0;
        }
        if min_separation_time_us < 0x7F * 1000 {
            return (min_separation_time_us / 1000) as u8;
        }
        0x7F
    }
}

// Ported from docan/test/src/docan/common/DoCanParametersTest.cpp,
// DoCanTransportAddressPairTest.cpp, DoCanConnectionTest.cpp and
// datalink/DoCanDataLinkAddressPairTest.cpp.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{FdFrameSizeMapper, presets};

    fn system_us() -> u32 {
        0
    }

    #[test]
    fn constructed_parameters_and_setter() {
        let cut = DoCanParameters::new(&system_us, 23425, 3543, 1232, 3442, 98, 153, 75000, 122);
        assert_eq!(cut.wait_allocate_timeout(), 23425);
        assert_eq!(cut.wait_rx_timeout(), 3543);
        assert_eq!(cut.wait_tx_callback_timeout(), 1232);
        assert_eq!(cut.wait_flow_control_timeout(), 3442);
        assert_eq!(cut.max_allocate_retry_count(), 98);
        assert_eq!(cut.max_flow_control_wait_count(), 153);
        assert_eq!(
            cut.encoded_min_separation_time(),
            DoCanParameters::encode_min_separation_time(75000)
        );
        assert_eq!(cut.max_block_size(), 122);
        cut.set_max_block_size(167);
        assert_eq!(cut.max_block_size(), 167);
        cut.set_encoded_min_separation_time(0x88);
        assert_eq!(cut.encoded_min_separation_time(), 0x88);
        assert_eq!(cut.now_us(), 0);
    }

    #[test]
    fn decode_min_separation_time() {
        assert_eq!(DoCanParameters::decode_min_separation_time(0x00), 0);
        assert_eq!(DoCanParameters::decode_min_separation_time(0x01), 1000);
        assert_eq!(DoCanParameters::decode_min_separation_time(0x7F), 0x7F * 1000);
        assert_eq!(DoCanParameters::decode_min_separation_time(0x80), 0x7F * 1000);
        assert_eq!(DoCanParameters::decode_min_separation_time(0xEF), 0x7F * 1000);
        assert_eq!(DoCanParameters::decode_min_separation_time(0xF0), 0x7F * 1000);
        for value in 1..=9u32 {
            assert_eq!(
                DoCanParameters::decode_min_separation_time(value as u8 + 0xF0),
                value * 100
            );
        }
        assert_eq!(DoCanParameters::decode_min_separation_time(0xFA), 0x7F * 1000);
    }

    #[test]
    fn encode_min_separation_time() {
        assert_eq!(DoCanParameters::encode_min_separation_time(0), 0x00);
        assert_eq!(DoCanParameters::encode_min_separation_time(1000), 0x01);
        assert_eq!(DoCanParameters::encode_min_separation_time(1500), 0x01);
        assert_eq!(DoCanParameters::encode_min_separation_time(0x7F * 1000), 0x7F);
        assert_eq!(DoCanParameters::encode_min_separation_time(0x7F * 1000 + 1500), 0x7F);
        assert_eq!(DoCanParameters::encode_min_separation_time(0x80 * 1000), 0x7F);
        for value in 1..=9u32 {
            assert_eq!(
                DoCanParameters::encode_min_separation_time(value * 100),
                value as u8 + 0xF0
            );
        }
        for t in 0..=(0x7F * 1000) {
            let result = DoCanParameters::encode_min_separation_time(t);
            assert!(result <= 0x7F || (0xF1..=0xF9).contains(&result));
        }
    }

    #[test]
    fn transport_address_pair() {
        let cut = TransportAddressPair::default();
        assert_eq!(cut.source_id(), 0);
        assert_eq!(cut.target_id(), 0);
        assert!(!cut.is_valid());
        let cut = TransportAddressPair::new(0x1234, 0x5678);
        assert!(cut.is_valid());
        assert_eq!(cut.invert(), TransportAddressPair::new(0x5678, 0x1234));
        assert_eq!(cut, TransportAddressPair::new(0x1234, 0x5678));
        assert_ne!(cut, TransportAddressPair::new(0x1234, 0x5679));
    }

    #[test]
    fn data_link_address_pair() {
        let cut = DataLinkAddressPair::default();
        assert!(!cut.is_valid());
        let cut = DataLinkAddressPair::new(0x123, 0x456);
        assert!(cut.is_valid());
        assert_eq!(cut.reception_address(), 0x123);
        assert_eq!(cut.transmission_address(), 0x456);
        assert_eq!(cut, DataLinkAddressPair::new(0x123, 0x456));
        assert_ne!(cut, DataLinkAddressPair::new(0x123, 0x457));
    }

    static MAPPER: FdFrameSizeMapper = FdFrameSizeMapper;
    static CODEC1: FrameCodec = FrameCodec::new(&presets::PADDED_CLASSIC, &MAPPER);
    static CODEC2: FrameCodec = FrameCodec::new(&presets::PADDED_FD, &MAPPER);

    #[test]
    fn connection() {
        let cut = Connection::new(
            &CODEC1,
            DataLinkAddressPair::new(1, 2),
            TransportAddressPair::new(3, 4),
        );
        assert!(core::ptr::eq(cut.frame_codec(), &CODEC1));
        assert_eq!(*cut.data_link_address_pair(), DataLinkAddressPair::new(1, 2));
        assert_eq!(*cut.transport_address_pair(), TransportAddressPair::new(3, 4));
        assert_eq!(
            cut,
            Connection::new(
                &CODEC1,
                DataLinkAddressPair::new(1, 2),
                TransportAddressPair::new(3, 4)
            )
        );
        assert_ne!(
            cut,
            Connection::new(
                &CODEC2,
                DataLinkAddressPair::new(1, 2),
                TransportAddressPair::new(3, 4)
            )
        );
        assert_ne!(
            cut,
            Connection::new(
                &CODEC1,
                DataLinkAddressPair::new(1, 3),
                TransportAddressPair::new(3, 4)
            )
        );
    }

    #[test]
    fn frame_type_and_flow_status() {
        assert_eq!(FrameType::from_nibble(0), Some(FrameType::Single));
        assert_eq!(FrameType::from_nibble(3), Some(FrameType::FlowControl));
        assert_eq!(FrameType::from_nibble(5), None);
        assert_eq!(FlowStatus::from_nibble(4), FlowStatus::Invalid(4));
        assert_eq!(FlowStatus::Ovflw.nibble(), 2);
        assert!(time_less(5, 10));
        assert!(!time_less(10, 5));
        assert!(time_less(u32::MAX, 2));
    }
}
