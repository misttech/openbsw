// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The CAN side of DoCAN, ported from `datalink/IDoCanDataFrameTransmitter.h`,
//! `IDoCanDataFrameTransmitterCallback.h`, `IDoCanFlowControlFrameTransmitter.h`,
//! `IDoCanFrameReceiver.h`, `IDoCanPhysicalTransceiver.h`, `transmitter/IDoCanTickGenerator.h`
//! and `can/DoCanPhysicalCanTransceiver.h`.

use core::cell::Cell;

use openbsw_cpp2can::filter::Filter;
use openbsw_cpp2can::{
    CanFrame, CanFrameListener, CanFrameSentListener, CanTransceiver, ErrorCode, ListenerNode,
};

use crate::addressing::{AddressConverter, decode_reception_address, encode_transmission_address};
use crate::codec::FrameCodec;
use crate::common::{
    Address, Connection, DataLinkAddressPair, FlowStatus, FrameIndex, FrameSize, JobHandle,
    MessageSize,
};
use crate::decoder::decode_frame;

/// What starting to send data frames reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendResult {
    /// Queued; more can be queued.
    Queued,
    /// Queued; nothing more fits now.
    QueuedFull,
    /// Nothing fits now.
    Full,
    /// The frame could not be encoded.
    Invalid,
    /// Sending failed.
    Failed,
}

/// Sends data frames: the port of `IDoCanDataFrameTransmitter`.
pub trait DataFrameTransmitter: Sync {
    /// Send frames `first_frame_index` to `last_frame_index` of `data`, telling `callback`
    /// with `job_handle` when they left.
    #[expect(clippy::too_many_arguments, reason = "the C++ interface's parameters")]
    fn start_send_data_frames(
        &self,
        codec: &FrameCodec,
        callback: &'static dyn DataFrameTransmitterCallback,
        job_handle: JobHandle,
        transmission_address: Address,
        first_frame_index: FrameIndex,
        last_frame_index: FrameIndex,
        consecutive_frame_data_size: FrameSize,
        data: &[Cell<u8>],
    ) -> SendResult;
    /// Forget the frames of `job_handle` queued for `callback`.
    fn cancel_send_data_frames(
        &self,
        callback: &'static dyn DataFrameTransmitterCallback,
        job_handle: JobHandle,
    );
}

/// Told when data frames left: the port of `IDoCanDataFrameTransmitterCallback`.
pub trait DataFrameTransmitterCallback: Sync {
    /// `frame_count` frames with `data_size` bytes of `job_handle` left.
    fn data_frames_sent(
        &self,
        job_handle: JobHandle,
        frame_count: FrameIndex,
        data_size: MessageSize,
    );
}

/// Sends flow control frames: the port of `IDoCanFlowControlFrameTransmitter`.
pub trait FlowControlFrameTransmitter: Sync {
    /// Send a flow control frame; returns whether it was queued.
    fn send_flow_control(
        &self,
        codec: &FrameCodec,
        transmission_address: Address,
        flow_status: FlowStatus,
        block_size: u8,
        encoded_min_separation_time: u8,
    ) -> bool;
}

/// Receives decoded frames: the port of `IDoCanFrameReceiver`.
pub trait FrameReceiver: Sync {
    /// A single or first frame of a message arrived.
    fn first_data_frame_received(
        &self,
        connection: &Connection,
        message_size: MessageSize,
        frame_count: FrameIndex,
        consecutive_frame_data_size: FrameSize,
        data: &[u8],
    );
    /// A consecutive frame arrived.
    fn consecutive_data_frame_received(
        &self,
        reception_address: Address,
        sequence_number: u8,
        data: &[u8],
    );
    /// A flow control frame arrived.
    fn flow_control_frame_received(
        &self,
        reception_address: Address,
        flow_status: FlowStatus,
        block_size: u8,
        encoded_min_separation_time: u8,
    );
}

/// The whole CAN side: the port of `IDoCanPhysicalTransceiver`.
pub trait PhysicalTransceiver: DataFrameTransmitter + FlowControlFrameTransmitter {
    /// Start, delivering received frames to `receiver`.
    fn init(&self, receiver: &'static dyn FrameReceiver);
    /// Stop.
    fn shutdown(&self);
}

/// Asks for a tick when consecutive frames are being paced: the port of `IDoCanTickGenerator`.
pub trait TickGenerator: Sync {
    /// Call `tick` soon.
    fn tick_needed(&self);
}

/// DoCAN over a `cpp2can` transceiver with normal addressing: the port of
/// `DoCanPhysicalCanTransceiver<DoCanNormalAddressing<>>`.
///
/// A `'static` object: it registers itself as the CAN frame listener and as the sent
/// listener of the frames it writes.
pub struct PhysicalCanTransceiver {
    this: &'static PhysicalCanTransceiver,
    frame: Cell<CanFrame>,
    transceiver: &'static dyn CanTransceiver,
    filter: &'static dyn Filter,
    address_converter: &'static dyn AddressConverter,
    frame_receiver: Cell<Option<&'static dyn FrameReceiver>>,
    send_callback: Cell<Option<&'static dyn DataFrameTransmitterCallback>>,
    send_job_handle: Cell<JobHandle>,
    send_data_size: Cell<u8>,
    send_pending: Cell<bool>,
    listener_node: ListenerNode,
}

// SAFETY: the cells are changed on the CAN context (writes and the sent callback) and the
// receive path, the same contexts the C++ object is used on.
unsafe impl Sync for PhysicalCanTransceiver {}

impl PhysicalCanTransceiver {
    /// A transceiver over `transceiver`, listening with `filter`, addressing through
    /// `address_converter`; `this` is its own `'static` address.
    pub const fn new(
        this: &'static PhysicalCanTransceiver,
        transceiver: &'static dyn CanTransceiver,
        filter: &'static dyn Filter,
        address_converter: &'static dyn AddressConverter,
    ) -> Self {
        Self {
            this,
            frame: Cell::new(CanFrame::new()),
            transceiver,
            filter,
            address_converter,
            frame_receiver: Cell::new(None),
            send_callback: Cell::new(None),
            send_job_handle: Cell::new(JobHandle::new(0, 0)),
            send_data_size: Cell::new(0),
            send_pending: Cell::new(false),
            listener_node: ListenerNode::new(),
        }
    }

    /// The CAN frame listener this transceiver is.
    pub fn listener(&self) -> &'static dyn CanFrameListener {
        self.this
    }
}

impl DataFrameTransmitter for PhysicalCanTransceiver {
    fn start_send_data_frames(
        &self,
        codec: &FrameCodec,
        callback: &'static dyn DataFrameTransmitterCallback,
        job_handle: JobHandle,
        transmission_address: Address,
        first_frame_index: FrameIndex,
        _last_frame_index: FrameIndex,
        consecutive_frame_data_size: FrameSize,
        data: &[Cell<u8>],
    ) -> SendResult {
        if self.send_pending.get() || data.len() > usize::from(MessageSize::MAX) {
            return SendResult::Full;
        }
        let mut frame = self.frame.get();
        let mut payload = [0u8; CanFrame::MAX_FRAME_LENGTH as usize];
        // One frame carries at most a frame's worth of the message.
        let mut head = [0u8; CanFrame::MAX_FRAME_LENGTH as usize];
        let head_length = data.len().min(head.len());
        for (byte, cell) in head.iter_mut().zip(data) {
            *byte = cell.get();
        }
        let Ok((length, consumed)) = codec.encode_data_frame_head(
            &mut payload,
            data.len() as MessageSize,
            &head[..head_length],
            first_frame_index,
            consecutive_frame_data_size,
        ) else {
            return SendResult::Invalid;
        };
        self.send_data_size.set(consumed);
        let can_id = encode_transmission_address(transmission_address, &mut payload[..length]);
        frame.set_id(can_id);
        frame.set_payload(&payload[..length]);
        self.frame.set(frame);
        let result = self.transceiver.write_with_listener(&mut frame, self.this);
        // The transceiver stamps the frame it sent, as the C++ member frame is stamped.
        self.frame.set(frame);
        match result {
            ErrorCode::Ok => {
                self.send_pending.set(true);
                self.send_callback.set(Some(callback));
                self.send_job_handle.set(job_handle);
                SendResult::QueuedFull
            }
            ErrorCode::TxHwQueueFull => SendResult::Full,
            _ => SendResult::Failed,
        }
    }

    fn cancel_send_data_frames(
        &self,
        callback: &'static dyn DataFrameTransmitterCallback,
        job_handle: JobHandle,
    ) {
        if self.send_pending.get()
            && self.send_callback.get().is_some_and(|current| core::ptr::addr_eq(current, callback))
            && self.send_job_handle.get() == job_handle
        {
            self.send_pending.set(false);
            self.send_callback.set(None);
        }
    }
}

impl FlowControlFrameTransmitter for PhysicalCanTransceiver {
    fn send_flow_control(
        &self,
        codec: &FrameCodec,
        transmission_address: Address,
        flow_status: FlowStatus,
        block_size: u8,
        encoded_min_separation_time: u8,
    ) -> bool {
        let mut payload = [0u8; CanFrame::MAX_FRAME_LENGTH as usize];
        let length = codec
            .encode_flow_control_frame(
                &mut payload,
                flow_status,
                block_size,
                encoded_min_separation_time,
            )
            .unwrap_or(0);
        let can_id = encode_transmission_address(transmission_address, &mut payload[..length]);
        let mut frame = CanFrame::new();
        frame.set_id(can_id);
        frame.set_payload(&payload[..length]);
        self.transceiver.write(&mut frame) == ErrorCode::Ok
    }
}

impl PhysicalTransceiver for PhysicalCanTransceiver {
    fn init(&self, receiver: &'static dyn FrameReceiver) {
        self.frame_receiver.set(Some(receiver));
        self.transceiver.add_can_frame_listener(self.this);
    }

    fn shutdown(&self) {
        self.frame_receiver.set(None);
        self.transceiver.remove_can_frame_listener(self.this);
    }
}

impl CanFrameListener for PhysicalCanTransceiver {
    fn frame_received(&self, can_frame: &CanFrame) {
        let Some(frame_receiver) = self.frame_receiver.get() else {
            return;
        };
        let payload = can_frame.payload();
        let reception_address = decode_reception_address(can_frame.id(), payload);
        let Some((codec, transport_address_pair, transmission_address)) =
            self.address_converter.reception_parameters(reception_address)
        else {
            return;
        };
        let connection = Connection::new(
            codec,
            DataLinkAddressPair::new(reception_address, transmission_address),
            transport_address_pair,
        );
        let _ = decode_frame(&connection, payload, frame_receiver);
    }

    fn filter(&self) -> &dyn Filter {
        self.filter
    }

    fn node(&self) -> &ListenerNode {
        &self.listener_node
    }
}

impl CanFrameSentListener for PhysicalCanTransceiver {
    fn can_frame_sent(&self, _frame: &CanFrame) {
        if self.send_pending.get() {
            self.send_pending.set(false);
            if let Some(callback) = self.send_callback.get() {
                callback.data_frames_sent(
                    self.send_job_handle.get(),
                    1,
                    MessageSize::from(self.send_data_size.get()),
                );
            }
        }
    }
}
