// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Decodes a received frame and hands it on, ported from `datalink/DoCanFrameDecoder.h`.

use crate::codec::CodecError;
use crate::common::{Connection, FrameType};
use crate::transceiver::FrameReceiver;

/// Decode `payload`, received on `connection`, and tell `receiver` what it was.
pub fn decode_frame(
    connection: &Connection,
    payload: &[u8],
    receiver: &dyn FrameReceiver,
) -> Result<(), CodecError> {
    let codec = connection.frame_codec();
    match codec.decode_frame_type(payload)? {
        FrameType::Single => {
            let (message_size, data) = codec.decode_single_frame(payload)?;
            receiver.first_data_frame_received(connection, message_size, 1, 0, data);
        }
        FrameType::First => {
            let first = codec.decode_first_frame(payload)?;
            receiver.first_data_frame_received(
                connection,
                first.message_size,
                first.frame_count,
                first.consecutive_frame_data_size,
                first.data,
            );
        }
        FrameType::Consecutive => {
            let (sequence_number, data) = codec.decode_consecutive_frame(payload)?;
            receiver.consecutive_data_frame_received(
                connection.data_link_address_pair().reception_address(),
                sequence_number,
                data,
            );
        }
        FrameType::FlowControl => {
            let (flow_status, block_size, encoded_min_separation_time) =
                codec.decode_flow_control_frame(payload)?;
            receiver.flow_control_frame_received(
                connection.data_link_address_pair().reception_address(),
                flow_status,
                block_size,
                encoded_min_separation_time,
            );
        }
    }
    Ok(())
}

// Ported from docan/test/src/docan/datalink/DoCanFrameDecoderTest.cpp.
#[cfg(test)]
pub(crate) mod tests {
    extern crate std;

    use std::cell::RefCell;
    use std::vec::Vec;

    use super::*;
    use crate::codec::{FdFrameSizeMapper, FrameCodec, presets};
    use crate::common::{
        Address, DataLinkAddressPair, FlowStatus, FrameIndex, FrameSize, MessageSize,
        TransportAddressPair,
    };

    /// Records what the decoder delivers: the port of `DoCanFrameReceiverMock`.
    #[derive(Debug, Clone, PartialEq)]
    pub(crate) enum Received {
        First(Connection, MessageSize, FrameIndex, FrameSize, Vec<u8>),
        Consecutive(Address, u8, Vec<u8>),
        FlowControl(Address, FlowStatus, u8, u8),
    }

    #[derive(Default)]
    pub(crate) struct Recorder(pub(crate) RefCell<Vec<Received>>);
    // SAFETY: single-threaded test object.
    unsafe impl Sync for Recorder {}

    impl Recorder {
        pub(crate) fn take(&self) -> Vec<Received> {
            core::mem::take(&mut *self.0.borrow_mut())
        }
    }

    impl FrameReceiver for Recorder {
        fn first_data_frame_received(
            &self,
            connection: &Connection,
            message_size: MessageSize,
            frame_count: FrameIndex,
            consecutive_frame_data_size: FrameSize,
            data: &[u8],
        ) {
            self.0.borrow_mut().push(Received::First(
                *connection,
                message_size,
                frame_count,
                consecutive_frame_data_size,
                data.to_vec(),
            ));
        }
        fn consecutive_data_frame_received(
            &self,
            reception_address: Address,
            sequence_number: u8,
            data: &[u8],
        ) {
            self.0.borrow_mut().push(Received::Consecutive(
                reception_address,
                sequence_number,
                data.to_vec(),
            ));
        }
        fn flow_control_frame_received(
            &self,
            reception_address: Address,
            flow_status: FlowStatus,
            block_size: u8,
            encoded_min_separation_time: u8,
        ) {
            self.0.borrow_mut().push(Received::FlowControl(
                reception_address,
                flow_status,
                block_size,
                encoded_min_separation_time,
            ));
        }
    }

    static MAPPER: FdFrameSizeMapper = FdFrameSizeMapper;
    static CODEC: FrameCodec = FrameCodec::new(&presets::OPTIMIZED_CLASSIC, &MAPPER);

    fn connection() -> Connection {
        Connection::new(
            &CODEC,
            DataLinkAddressPair::new(0x1, 0x3),
            TransportAddressPair::new(0x3, 0x4),
        )
    }

    #[test]
    fn decode_single_frame() {
        let recorder = Recorder::default();
        let conn = connection();
        assert_eq!(decode_frame(&conn, &[0x02, 0x13, 0x24], &recorder), Ok(()));
        assert_eq!(recorder.take(), [Received::First(conn, 2, 1, 0, std::vec![0x13, 0x24])]);
        assert_eq!(
            decode_frame(&conn, &[0x02, 0x24], &recorder),
            Err(CodecError::InvalidMessageSize)
        );
        assert!(recorder.take().is_empty());
    }

    #[test]
    fn decode_first_frame() {
        let recorder = Recorder::default();
        let conn = connection();
        let payload = [0x10, 0x12, 0x24, 0x45, 0x67, 0x89, 0x9a, 0x91];
        assert_eq!(decode_frame(&conn, &payload, &recorder), Ok(()));
        assert_eq!(recorder.take(), [Received::First(conn, 0x12, 3, 7, payload[2..].to_vec())]);
        let payload = [0x10, 0x00, 0x00, 0x00, 0xA5, 0xB4, 0x5A, 0x4B];
        assert_eq!(decode_frame(&conn, &payload, &recorder), Ok(()));
        assert_eq!(
            recorder.take(),
            [Received::First(conn, 0xA5B4, 6061, 7, payload[6..].to_vec())]
        );
        assert_eq!(
            decode_frame(&conn, &[0x12, 0x24], &recorder),
            Err(CodecError::InvalidFrameSize)
        );
    }

    #[test]
    fn decode_consecutive_frame() {
        let recorder = Recorder::default();
        let conn = connection();
        let payload = [0x21, 0x12, 0x24, 0x45, 0x67, 0x89, 0x9a, 0x91];
        assert_eq!(decode_frame(&conn, &payload, &recorder), Ok(()));
        assert_eq!(recorder.take(), [Received::Consecutive(0x01, 1, payload[1..].to_vec())]);
        assert_eq!(decode_frame(&conn, &[0x21], &recorder), Err(CodecError::InvalidFrameSize));
    }

    #[test]
    fn decode_flow_control_frame() {
        let recorder = Recorder::default();
        let conn = connection();
        assert_eq!(decode_frame(&conn, &[0x30, 0x12, 0x24], &recorder), Ok(()));
        assert_eq!(recorder.take(), [Received::FlowControl(0x1, FlowStatus::Cts, 0x12, 0x24)]);
        assert_eq!(
            decode_frame(&conn, &[0x34, 0x12], &recorder),
            Err(CodecError::InvalidFrameSize)
        );
    }

    #[test]
    fn decode_frame_with_invalid_frame_and_unknown_type() {
        let recorder = Recorder::default();
        let conn = connection();
        assert_eq!(decode_frame(&conn, &[], &recorder), Err(CodecError::InvalidFrameSize));
        assert_eq!(
            decode_frame(&conn, &[0x50, 0x12], &recorder),
            Err(CodecError::InvalidFrameType)
        );
        assert!(recorder.take().is_empty());
    }
}
