// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The receiving side, ported from `receiver/DoCanMessageReceiveProtocolHandler.h`,
//! `DoCanMessageReceiver.h` and `DoCanReceiver.h`: one state machine per message being
//! received, in a pool of slots, driven by the frames that arrive, the buffers the
//! transport provides and the timeouts.

use core::cell::Cell;
use core::marker::PhantomData;

use openbsw_async::{ContextType, QueueNode, Runnable};
use openbsw_timer::Lock;
use openbsw_transport::{
    ProcessingResult, ProviderErrorCode, ReceiveResult as TransportReceiveResult, TransportMessage,
    TransportMessageProcessedListener, TransportMessageProvidingListener, bus_name,
};
use openbsw_util::format::Arg;
use openbsw_util::logger::LoggerComponent;
use openbsw_util::{log_error, log_info, log_warn};

use crate::addressing::AddressConverter;
use crate::codec::FrameCodec;
use crate::common::{
    Address, Connection, DoCanParameters, FlowStatus, FrameIndex, FrameSize, MessageSize,
    TransportAddressPair, time_less,
};
use crate::transceiver::FlowControlFrameTransmitter;

/// The most first-frame data a receiver keeps (the config's `MaxFrameSize`).
pub const MAX_FIRST_FRAME_DATA_SIZE: usize = 64;
const FORMAT_BUFFER_SIZE: usize = 32;

/// Where a reception stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiveState {
    /// Waiting for a transport buffer.
    Allocate = 0,
    /// Waiting for a frame or a retry.
    Wait = 1,
    /// A flow control frame is due.
    Send = 2,
    /// The message is with the application.
    Processing = 3,
    /// Finished.
    Done = 4,
}

/// What a waiting reception waits for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiveTimeout {
    /// Nothing.
    None,
    /// The next consecutive frame.
    Rx,
    /// A retry of the buffer allocation.
    Allocate,
}

/// What a transition reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiveMessage {
    /// Nothing to say.
    None,
    /// The call did not fit the state.
    IllegalState,
    /// No buffer came in time.
    AllocationRetryCountExceeded,
    /// No consecutive frame came in time.
    RxTimeoutExpired,
    /// A consecutive frame was out of sequence.
    BadSequenceNumber,
    /// The application refused the message.
    ProcessingFailed,
}

/// The outcome of a protocol handler call: whether the state changed, and a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReceiveResult {
    transition: bool,
    message: ReceiveMessage,
    param: u8,
}

impl ReceiveResult {
    /// A result with or without a transition.
    pub const fn new(transition: bool) -> Self {
        Self { transition, message: ReceiveMessage::None, param: 0 }
    }

    /// With `message`.
    pub const fn with_message(self, message: ReceiveMessage) -> Self {
        Self { message, param: 0, ..self }
    }

    /// With `message` and `param`.
    pub const fn with_message_param(self, message: ReceiveMessage, param: u8) -> Self {
        Self { message, param, ..self }
    }

    /// Whether the state changed.
    pub const fn has_transition(&self) -> bool {
        self.transition
    }

    /// The message.
    pub const fn message(&self) -> ReceiveMessage {
        self.message
    }

    /// The message's parameter.
    pub const fn param(&self) -> u8 {
        self.param
    }
}

/// The state machine of one reception: the port of `DoCanMessageReceiveProtocolHandler`.
pub struct ReceiveProtocolHandler {
    frame_index: Cell<FrameIndex>,
    frame_count: Cell<FrameIndex>,
    state: Cell<ReceiveState>,
    timeout: Cell<ReceiveTimeout>,
    block_frame_index: Cell<u8>,
    allocate_retry_count: Cell<u8>,
    is_allocating: Cell<bool>,
}

// SAFETY: driven under the receiver's lock.
unsafe impl Sync for ReceiveProtocolHandler {}

impl ReceiveProtocolHandler {
    /// A handler for a message of `frame_count` frames, about to allocate.
    pub const fn new(frame_count: FrameIndex) -> Self {
        Self {
            frame_index: Cell::new(1),
            frame_count: Cell::new(frame_count),
            state: Cell::new(ReceiveState::Allocate),
            timeout: Cell::new(ReceiveTimeout::None),
            block_frame_index: Cell::new(0),
            allocate_retry_count: Cell::new(0),
            is_allocating: Cell::new(true),
        }
    }

    /// Start over for a message of `frame_count` frames.
    pub fn reset(&self, frame_count: FrameIndex) {
        self.frame_index.set(1);
        self.frame_count.set(frame_count);
        self.state.set(ReceiveState::Allocate);
        self.timeout.set(ReceiveTimeout::None);
        self.block_frame_index.set(0);
        self.allocate_retry_count.set(0);
        self.is_allocating.set(true);
    }

    /// The state.
    pub fn state(&self) -> ReceiveState {
        self.state.get()
    }

    /// What is waited for.
    pub fn timeout(&self) -> ReceiveTimeout {
        self.timeout.get()
    }

    /// The next frame's index.
    pub fn frame_index(&self) -> FrameIndex {
        self.frame_index.get()
    }

    /// The message's frame count.
    pub fn frame_count(&self) -> FrameIndex {
        self.frame_count.get()
    }

    /// Whether the next flow control must say `WAIT` (no buffer yet for a segmented message).
    pub fn is_flow_control_wait(&self) -> bool {
        self.allocate_retry_count.get() > 0 && self.frame_count.get() > 1
    }

    /// Whether a buffer is still wanted.
    pub fn is_allocating(&self) -> bool {
        self.is_allocating.get()
    }

    /// Finish with `message`.
    pub fn cancel(&self, message: ReceiveMessage) -> ReceiveResult {
        self.set_done(message, 0)
    }

    /// Finish unless the application holds the message.
    pub fn shutdown(&self) -> ReceiveResult {
        if self.state.get() == ReceiveState::Processing {
            ReceiveResult::new(false)
        } else {
            self.cancel(ReceiveMessage::None)
        }
    }

    /// A buffer was (`success`) or was not provided.
    pub fn allocated(&self, success: bool, max_retry_count: u8) -> ReceiveResult {
        if !self.is_allocating.get() {
            return ReceiveResult::new(true)
                .with_message_param(ReceiveMessage::IllegalState, self.state.get() as u8);
        }
        if success {
            self.is_allocating.set(false);
            self.allocate_retry_count.set(0);
            return self.set_state(
                if self.frame_count.get() == 1 {
                    ReceiveState::Processing
                } else {
                    ReceiveState::Send
                },
                ReceiveTimeout::None,
            );
        }
        if self.frame_count.get() == 1 {
            self.allocate_retry_count.set(self.allocate_retry_count.get() + 1);
            if self.allocate_retry_count.get() > 1 {
                return self.set_done(ReceiveMessage::AllocationRetryCountExceeded, 0);
            }
            return self.set_state(ReceiveState::Wait, ReceiveTimeout::Allocate);
        }
        self.allocate_retry_count.set(self.allocate_retry_count.get() + 1);
        if self.allocate_retry_count.get() > max_retry_count {
            return self.set_done(ReceiveMessage::AllocationRetryCountExceeded, 0);
        }
        self.set_state(ReceiveState::Send, ReceiveTimeout::None)
    }

    /// The flow control frame was (`success`) or was not sent.
    pub fn frame_sent(&self, success: bool) -> ReceiveResult {
        if self.state.get() != ReceiveState::Send {
            return ReceiveResult::new(true)
                .with_message_param(ReceiveMessage::IllegalState, self.state.get() as u8);
        }
        if success {
            return self.set_state(
                ReceiveState::Wait,
                if self.is_flow_control_wait() {
                    ReceiveTimeout::Allocate
                } else {
                    ReceiveTimeout::Rx
                },
            );
        }
        ReceiveResult::new(false)
    }

    /// A consecutive frame with `frame_index` (its low nibble) arrived.
    pub fn consecutive_frame_received(&self, frame_index: u8, max_block_size: u8) -> ReceiveResult {
        if self.state.get() != ReceiveState::Wait || self.timeout.get() != ReceiveTimeout::Rx {
            return ReceiveResult::new(false);
        }
        if (self.frame_index.get() & 0xF) as u8 != frame_index {
            return self.set_done(ReceiveMessage::BadSequenceNumber, frame_index);
        }
        self.frame_index.set(self.frame_index.get() + 1);
        if self.frame_index.get() == self.frame_count.get() {
            return self.set_state(ReceiveState::Processing, ReceiveTimeout::None);
        }
        if max_block_size > 0 {
            self.block_frame_index.set(self.block_frame_index.get() + 1);
            if self.block_frame_index.get() == max_block_size {
                self.block_frame_index.set(0);
                return self.set_state(ReceiveState::Send, ReceiveTimeout::None);
            }
        }
        self.set_state(ReceiveState::Wait, ReceiveTimeout::Rx)
    }

    /// The application finished with the message.
    pub fn processed(&self, success: bool) -> ReceiveResult {
        self.set_done(
            if success { ReceiveMessage::None } else { ReceiveMessage::ProcessingFailed },
            0,
        )
    }

    /// The timeout expired.
    pub fn expired(&self) -> ReceiveResult {
        match self.timeout.get() {
            ReceiveTimeout::Rx => self.set_done(ReceiveMessage::RxTimeoutExpired, 0),
            ReceiveTimeout::Allocate => {
                self.set_state(ReceiveState::Allocate, ReceiveTimeout::None)
            }
            ReceiveTimeout::None => ReceiveResult::new(false),
        }
    }

    fn set_state(&self, state: ReceiveState, timeout: ReceiveTimeout) -> ReceiveResult {
        self.state.set(state);
        self.timeout.set(timeout);
        ReceiveResult::new(true)
    }

    fn set_done(&self, message: ReceiveMessage, param: u8) -> ReceiveResult {
        self.state.set(ReceiveState::Done);
        self.timeout.set(ReceiveTimeout::None);
        self.is_allocating.set(false);
        ReceiveResult::new(true).with_message_param(message, param)
    }
}

/// One reception: the handler plus the message, the first frame's data and the timer
/// (`DoCanMessageReceiver`), in a slot of a [`ReceiverPool`].
pub struct MessageReceiver {
    handler: ReceiveProtocolHandler,
    used: Cell<bool>,
    connection: Cell<Option<Connection>>,
    message: Cell<Option<&'static TransportMessage>>,
    first_frame_data: [Cell<u8>; MAX_FIRST_FRAME_DATA_SIZE],
    first_frame_data_size: Cell<FrameSize>,
    timer: Cell<u32>,
    message_size: Cell<MessageSize>,
    consecutive_frame_data_size: Cell<FrameSize>,
    is_timer_set: Cell<bool>,
    max_block_size: Cell<u8>,
    encoded_min_separation_time: Cell<u8>,
    blocked: Cell<bool>,
}

// SAFETY: driven under the receiver's lock.
unsafe impl Sync for MessageReceiver {}

impl MessageReceiver {
    /// An unused slot.
    pub const fn new() -> Self {
        Self {
            handler: ReceiveProtocolHandler::new(0),
            used: Cell::new(false),
            connection: Cell::new(None),
            message: Cell::new(None),
            first_frame_data: [const { Cell::new(0) }; MAX_FIRST_FRAME_DATA_SIZE],
            first_frame_data_size: Cell::new(0),
            timer: Cell::new(0),
            message_size: Cell::new(0),
            consecutive_frame_data_size: Cell::new(0),
            is_timer_set: Cell::new(false),
            max_block_size: Cell::new(0),
            encoded_min_separation_time: Cell::new(0),
            blocked: Cell::new(false),
        }
    }

    /// Take the slot for a new reception (the C++ constructor).
    #[expect(clippy::too_many_arguments, reason = "the C++ constructor's parameters")]
    pub fn start(
        &self,
        connection: Connection,
        message_size: MessageSize,
        frame_count: FrameIndex,
        consecutive_frame_data_size: FrameSize,
        max_block_size: u8,
        encoded_min_separation_time: u8,
        first_frame_data: &[u8],
        blocked: bool,
    ) {
        self.handler.reset(frame_count);
        self.used.set(true);
        self.connection.set(Some(connection));
        self.message.set(None);
        for (cell, &byte) in self.first_frame_data.iter().zip(first_frame_data) {
            cell.set(byte);
        }
        self.first_frame_data_size.set(first_frame_data.len() as FrameSize);
        self.timer.set(0);
        self.message_size.set(message_size);
        self.consecutive_frame_data_size.set(consecutive_frame_data_size);
        self.is_timer_set.set(false);
        self.max_block_size.set(max_block_size);
        self.encoded_min_separation_time.set(encoded_min_separation_time);
        self.blocked.set(blocked);
    }

    /// The state machine.
    pub fn handler(&self) -> &ReceiveProtocolHandler {
        &self.handler
    }

    /// Whether the slot holds a reception.
    pub fn is_used(&self) -> bool {
        self.used.get()
    }

    /// The transport message, once allocated.
    pub fn message(&self) -> Option<&'static TransportMessage> {
        self.message.get()
    }

    fn connection(&self) -> Connection {
        self.connection.get().expect("a used receiver has a connection")
    }

    /// The transport addresses.
    pub fn transport_address_pair(&self) -> TransportAddressPair {
        *self.connection().transport_address_pair()
    }

    /// The CAN id frames arrive on.
    pub fn reception_address(&self) -> Address {
        self.connection().data_link_address_pair().reception_address()
    }

    /// The CAN id flow control goes to.
    pub fn transmission_address(&self) -> Address {
        self.connection().data_link_address_pair().transmission_address()
    }

    /// The codec.
    pub fn frame_codec(&self) -> &'static FrameCodec {
        self.connection().frame_codec()
    }

    /// The message's size.
    pub fn message_size(&self) -> MessageSize {
        self.message_size.get()
    }

    /// The block size to ask for.
    pub fn max_block_size(&self) -> u8 {
        self.max_block_size.get()
    }

    /// The separation time to ask for, encoded.
    pub fn encoded_min_separation_time(&self) -> u8 {
        self.encoded_min_separation_time.get()
    }

    /// A buffer was provided (or not); the first frame's data goes in.
    pub fn allocated(
        &self,
        message: Option<&'static TransportMessage>,
        max_retry_count: u8,
    ) -> ReceiveResult {
        let result = self.handler.allocated(message.is_some(), max_retry_count);
        if let Some(message) = message
            && !self.handler.is_allocating()
        {
            self.message.set(Some(message));
            let mut data = [0u8; MAX_FIRST_FRAME_DATA_SIZE];
            let size = usize::from(self.first_frame_data_size.get());
            for (byte, cell) in data.iter_mut().zip(self.first_frame_data.iter()) {
                *byte = cell.get();
            }
            let _ = message.append(&data[..size]);
        }
        result
    }

    /// Whether consecutive frames are wanted.
    pub fn is_consecutive_frame_expected(&self) -> bool {
        self.message.get().is_some()
    }

    /// How many data bytes the next consecutive frame carries.
    pub fn expected_consecutive_frame_data_size(&self) -> FrameSize {
        let valid = self.message.get().map_or(0, |message| message.valid_bytes());
        if valid + MessageSize::from(self.consecutive_frame_data_size.get())
            <= self.message_size.get()
        {
            self.consecutive_frame_data_size.get()
        } else {
            (self.message_size.get() - valid) as FrameSize
        }
    }

    /// A consecutive frame arrived; `expected_size` of `data` goes into the message.
    pub fn consecutive_frame_received(
        &self,
        sequence_number: u8,
        expected_size: FrameSize,
        data: &[u8],
    ) -> ReceiveResult {
        let result =
            self.handler.consecutive_frame_received(sequence_number, self.max_block_size.get());
        if self.handler.state() != ReceiveState::Done
            && let Some(message) = self.message.get()
        {
            let _ = message.append(&data[..usize::from(expected_size).min(data.len())]);
        }
        result
    }

    /// Take the message away from the receiver.
    pub fn detach_message(&self) -> Option<&'static TransportMessage> {
        self.message.take()
    }

    /// Free the slot's message.
    pub fn release(&self) -> Option<&'static TransportMessage> {
        self.detach_message()
    }

    /// Whether the timer is at or past `now_us`.
    pub fn timer_expired(&self, now_us: u32) -> bool {
        time_less(self.timer.get(), now_us.wrapping_add(1))
    }

    /// Arm the timer for `next_expiry_us`.
    pub fn set_timer(&self, next_expiry_us: u32) {
        self.timer.set(next_expiry_us);
        self.is_timer_set.set(true);
    }

    /// Whether the armed timer expired at `now_us`; it is disarmed if so.
    pub fn update_timer(&self, now_us: u32) -> bool {
        if !self.is_timer_set.get() {
            return false;
        }
        if self.timer_expired(now_us) {
            self.is_timer_set.set(false);
            return true;
        }
        false
    }

    /// Whether this reception waits for an earlier one on the same address.
    pub fn is_blocked(&self) -> bool {
        self.blocked.get()
    }

    /// Set whether this reception waits.
    pub fn set_blocked(&self, blocked: bool) {
        self.blocked.set(blocked);
    }

    /// Copy the first frame's data into `buffer`; returns its size.
    pub fn first_frame_data(&self, buffer: &mut [u8; MAX_FIRST_FRAME_DATA_SIZE]) -> usize {
        for (byte, cell) in buffer.iter_mut().zip(self.first_frame_data.iter()) {
            *byte = cell.get();
        }
        usize::from(self.first_frame_data_size.get())
    }

    /// Sort key: receivers with a timer first, earliest first (`operator<`).
    fn sorts_before(&self, other: &MessageReceiver) -> bool {
        if !self.is_timer_set.get() {
            return false;
        }
        if !other.is_timer_set.get() {
            return true;
        }
        time_less(self.timer.get(), other.timer.get())
    }
}

impl Default for MessageReceiver {
    fn default() -> Self {
        Self::new()
    }
}

/// The receivers a layer can hold at once: the port of the message receiver pool and the
/// list over it, as `N` slots and their order.
pub struct ReceiverPool<const N: usize> {
    slots: [MessageReceiver; N],
    order: [Cell<u8>; N],
    count: Cell<usize>,
}

// SAFETY: changed under the receiver's lock.
unsafe impl<const N: usize> Sync for ReceiverPool<N> {}

impl<const N: usize> ReceiverPool<N> {
    /// An empty pool.
    pub const fn new() -> Self {
        assert!(N <= 255, "a receiver pool holds at most 255 slots");
        Self {
            slots: [const { MessageReceiver::new() }; N],
            order: [const { Cell::new(0) }; N],
            count: Cell::new(0),
        }
    }
}

impl<const N: usize> Default for ReceiverPool<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// What the receiver asks of its pool, whatever its size.
#[expect(clippy::len_without_is_empty, reason = "`is_full` is the question the pool answers")]
pub trait ReceiverSlots: Sync {
    /// Whether no slot is free.
    fn is_full(&self) -> bool;
    /// Take a free slot and put it at the end of the list.
    fn allocate(&self) -> Option<&'static MessageReceiver>;
    /// How many receivers are listed.
    fn len(&self) -> usize;
    /// The receiver at `index` of the list.
    fn at(&self, index: usize) -> &'static MessageReceiver;
    /// Free the receiver at `index` of the list.
    fn remove(&self, index: usize);
    /// Order the list by timer.
    fn sort(&self);
}

impl<const N: usize> ReceiverSlots for ReceiverPool<N> {
    fn is_full(&self) -> bool {
        self.count.get() == N
    }

    fn allocate(&self) -> Option<&'static MessageReceiver> {
        let (index, slot) = self.slots.iter().enumerate().find(|(_, slot)| !slot.is_used())?;
        slot.used.set(true);
        let count = self.count.get();
        self.order[count].set(index as u8);
        self.count.set(count + 1);
        // SAFETY: a pool is a `static` (its layer config is), so its slots live forever.
        Some(unsafe { &*(slot as *const MessageReceiver) })
    }

    fn len(&self) -> usize {
        self.count.get()
    }

    fn at(&self, index: usize) -> &'static MessageReceiver {
        let slot = &self.slots[usize::from(self.order[index].get())];
        // SAFETY: as in `allocate`.
        unsafe { &*(slot as *const MessageReceiver) }
    }

    fn remove(&self, index: usize) {
        let count = self.count.get();
        let slot = usize::from(self.order[index].get());
        self.slots[slot].used.set(false);
        for position in index..count - 1 {
            self.order[position].set(self.order[position + 1].get());
        }
        self.count.set(count - 1);
    }

    fn sort(&self) {
        let count = self.count.get();
        for i in 1..count {
            let mut j = i;
            while j > 0 {
                let a = &self.slots[usize::from(self.order[j].get())];
                let b = &self.slots[usize::from(self.order[j - 1].get())];
                if a.sorts_before(b) {
                    Cell::swap(&self.order[j], &self.order[j - 1]);
                    j -= 1;
                } else {
                    break;
                }
            }
        }
    }
}

/// The runnable that retries allocations once a message was processed.
struct ProcessReceivers<L: Lock + 'static> {
    owner: &'static DoCanReceiver<L>,
    node: QueueNode<dyn Runnable>,
}

impl<L: Lock + 'static> Runnable for ProcessReceivers<L> {
    fn execute(&self) {
        self.owner.process_message_receivers();
    }

    fn node(&self) -> &QueueNode<dyn Runnable> {
        &self.node
    }
}

/// Receives messages on one bus: the port of `DoCanReceiver`.
pub struct DoCanReceiver<L: Lock + 'static> {
    address_converter: &'static dyn AddressConverter,
    message_providing_listener: &'static dyn TransportMessageProvidingListener,
    flow_control_frame_transmitter: &'static dyn FlowControlFrameTransmitter,
    pool: &'static dyn ReceiverSlots,
    process_message_receivers: ProcessReceivers<L>,
    parameters: &'static DoCanParameters,
    context: ContextType,
    bus_id: u8,
    logger: &'static LoggerComponent,
    remove_lock_count: Cell<u8>,
    released_receiver_count: Cell<u8>,
    timers_updated: Cell<bool>,
    _lock: PhantomData<fn() -> L>,
}

// SAFETY: the counters change under `L` or on the receiver's context, as the C++ does.
unsafe impl<L: Lock + 'static> Sync for DoCanReceiver<L> {}

/// Holds receptions in place while they are walked (`RemoveGuard`).
struct RemoveGuard<'a, L: Lock + 'static>(&'a DoCanReceiver<L>);

impl<L: Lock + 'static> Drop for RemoveGuard<'_, L> {
    fn drop(&mut self) {
        self.0.release_remove_lock();
    }
}

impl<L: Lock + 'static> DoCanReceiver<L> {
    /// A receiver on `bus_id`, running on `context`, given its own `'static` address.
    #[expect(clippy::too_many_arguments, reason = "the C++ constructor's parameters")]
    pub const fn new(
        this: &'static DoCanReceiver<L>,
        bus_id: u8,
        context: ContextType,
        message_providing_listener: &'static dyn TransportMessageProvidingListener,
        flow_control_frame_transmitter: &'static dyn FlowControlFrameTransmitter,
        pool: &'static dyn ReceiverSlots,
        address_converter: &'static dyn AddressConverter,
        parameters: &'static DoCanParameters,
        logger: &'static LoggerComponent,
    ) -> Self {
        Self {
            address_converter,
            message_providing_listener,
            flow_control_frame_transmitter,
            pool,
            process_message_receivers: ProcessReceivers { owner: this, node: QueueNode::new() },
            parameters,
            context,
            bus_id,
            logger,
            remove_lock_count: Cell::new(0),
            released_receiver_count: Cell::new(0),
            timers_updated: Cell::new(false),
            _lock: PhantomData,
        }
    }

    /// Nothing to check: the pool's slots fit by construction.
    pub fn init(&self) {}

    /// Finish every reception the application does not hold.
    pub fn shutdown(&self) {
        let _guard = self.remove_guard();
        for index in 0..self.pool.len() {
            let receiver = self.pool.at(index);
            let _lock = L::lock();
            self.handle_transitions(receiver, receiver.handler().shutdown(), b"shutdown");
        }
    }

    /// A single or first frame arrived.
    pub fn first_data_frame_received(
        &self,
        connection: &Connection,
        message_size: MessageSize,
        frame_count: FrameIndex,
        consecutive_frame_data_size: FrameSize,
        data: &[u8],
    ) {
        let data_link_address_pair = connection.data_link_address_pair();
        let mut format_buffer = [0u8; FORMAT_BUFFER_SIZE];
        if frame_count > 1
            && data_link_address_pair.transmission_address() == crate::common::INVALID_ADDRESS
        {
            let address = self.address_converter.format_data_link_address(
                data_link_address_pair.reception_address(),
                &mut format_buffer,
            );
            log_warn!(
                self.logger,
                b"DoCanReceiver(%s)::firstDataFrameReceived(%s): functional first frames (RA: 0x%x) are not allowed!",
                Arg::Str(Some(self.name())),
                Arg::Str(Some(address)),
                data_link_address_pair.reception_address()
            );
        } else if !self.pool.is_full() {
            let _guard = self.remove_guard();
            if data.len() <= MAX_FIRST_FRAME_DATA_SIZE {
                let receiver = {
                    let _lock = L::lock();
                    let blocked = self.handle_pending_message_receivers(
                        data_link_address_pair.reception_address(),
                    );
                    let Some(receiver) = self.pool.allocate() else {
                        return;
                    };
                    receiver.start(
                        *connection,
                        message_size,
                        frame_count,
                        consecutive_frame_data_size,
                        self.parameters.max_block_size(),
                        self.parameters.encoded_min_separation_time(),
                        data,
                        blocked,
                    );
                    receiver
                };
                self.handle_transitions(
                    receiver,
                    ReceiveResult::new(true),
                    b"firstDataFrameReceived",
                );
            } else {
                let address = self.address_converter.format_data_link_address(
                    data_link_address_pair.reception_address(),
                    &mut format_buffer,
                );
                log_error!(
                    self.logger,
                    b"DoCanReceiver(%s)::firstDataFrameReceived(%s): Received %d bytes, expected max %d bytes!",
                    Arg::Str(Some(self.name())),
                    Arg::Str(Some(address)),
                    data.len(),
                    MAX_FIRST_FRAME_DATA_SIZE
                );
            }
        } else {
            let address = self.address_converter.format_data_link_address(
                data_link_address_pair.reception_address(),
                &mut format_buffer,
            );
            log_warn!(
                self.logger,
                b"DoCanReceiver(%s)::firstDataFrameReceived(%s): No empty message receiver found!",
                Arg::Str(Some(self.name())),
                Arg::Str(Some(address))
            );
        }
    }

    /// A consecutive frame arrived.
    pub fn consecutive_data_frame_received(
        &self,
        reception_address: Address,
        sequence_number: u8,
        data: &[u8],
    ) {
        let mut format_buffer = [0u8; FORMAT_BUFFER_SIZE];
        let receiver = self.find_message_receiver(reception_address);
        let Some(receiver) = receiver.filter(|receiver| receiver.is_consecutive_frame_expected())
        else {
            let address = self
                .address_converter
                .format_data_link_address(reception_address, &mut format_buffer);
            log_warn!(
                self.logger,
                b"DoCanReceiver(%s)::consecutiveDataFrameReceived(%s): unexpected consecutive frame!",
                Arg::Str(Some(self.name())),
                Arg::Str(Some(address))
            );
            return;
        };
        let expected_size = receiver.expected_consecutive_frame_data_size();
        if data.len() < usize::from(expected_size) {
            let address = self
                .address_converter
                .format_data_link_address(reception_address, &mut format_buffer);
            log_warn!(
                self.logger,
                b"DoCanReceiver(%s)::consecutiveDataFrameReceived(%s): consecutive frame length (%d) less than expected (%d)!",
                Arg::Str(Some(self.name())),
                Arg::Str(Some(address)),
                data.len(),
                expected_size
            );
            return;
        }
        let _guard = self.remove_guard();
        self.handle_transitions(
            receiver,
            receiver.consecutive_frame_received(sequence_number, expected_size, data),
            b"consecutiveDataFrameReceived",
        );
    }

    /// Expire the timers that are due at `now_us`.
    pub fn cyclic_task(&self, now_us: u32) {
        {
            let _guard = self.remove_guard();
            for index in 0..self.pool.len() {
                let receiver = self.pool.at(index);
                if !receiver.update_timer(now_us) {
                    break;
                }
                let _lock = L::lock();
                self.handle_transitions(receiver, receiver.handler().expired(), b"cyclicTask");
            }
        }
        if self.timers_updated.get() {
            let _lock = L::lock();
            self.pool.sort();
            self.timers_updated.set(false);
        }
    }

    fn process_message_receivers(&self) {
        let _guard = self.remove_guard();
        for index in 0..self.pool.len() {
            let receiver = self.pool.at(index);
            let _lock = L::lock();
            if receiver.handler().is_allocating() {
                self.handle_transitions(
                    receiver,
                    self.allocate_transport_message(receiver, true),
                    b"processMessageReceivers",
                );
                if receiver.message().is_none() {
                    break;
                }
            }
        }
    }

    fn handle_transitions(
        &self,
        receiver: &'static MessageReceiver,
        mut result: ReceiveResult,
        function_name: &[u8],
    ) {
        while result.has_transition() {
            self.handle_result(receiver, result, function_name);
            result = self.handle_transition(receiver);
        }
    }

    fn handle_result(
        &self,
        receiver: &MessageReceiver,
        result: ReceiveResult,
        function_name: &[u8],
    ) {
        if result.has_transition() {
            self.reset_timer(receiver);
            if receiver.handler().state() != ReceiveState::Done {
                self.timers_updated.set(true);
            }
        }
        if result.message() == ReceiveMessage::None {
            return;
        }
        let mut buffer = [0u8; FORMAT_BUFFER_SIZE];
        let address = self
            .address_converter
            .format_data_link_address(receiver.reception_address(), &mut buffer);
        let name = Arg::Str(Some(self.name()));
        let function = Arg::Str(Some(function_name));
        let address = Arg::Str(Some(address));
        match result.message() {
            ReceiveMessage::IllegalState => log_warn!(
                self.logger,
                b"DoCanReceiver(%s)::%s(%s): Illegal state 0x%x!",
                name,
                function,
                address,
                result.param()
            ),
            ReceiveMessage::AllocationRetryCountExceeded => {
                log_warn!(
                    self.logger,
                    b"DoCanReceiver(%s)::%s(%s): Allocation retry count exceeded",
                    name,
                    function,
                    address
                )
            }
            ReceiveMessage::RxTimeoutExpired => log_warn!(
                self.logger,
                b"DoCanReceiver(%s)::%s(%s): Rx timeout",
                name,
                function,
                address
            ),
            ReceiveMessage::BadSequenceNumber => {
                log_warn!(
                    self.logger,
                    b"DoCanReceiver(%s)::%s(%s): Frame with bad sequence number received",
                    name,
                    function,
                    address
                )
            }
            ReceiveMessage::ProcessingFailed => {
                log_warn!(
                    self.logger,
                    b"DoCanReceiver(%s)::%s(%s): Processing failed",
                    name,
                    function,
                    address
                )
            }
            ReceiveMessage::None => {}
        }
    }

    fn handle_transition(&self, receiver: &'static MessageReceiver) -> ReceiveResult {
        match receiver.handler().state() {
            ReceiveState::Allocate => self.allocate_transport_message(receiver, false),
            ReceiveState::Send => self.send_flow_control_frame(receiver),
            ReceiveState::Processing => self.start_processing_transport_message(receiver),
            ReceiveState::Done => self.release(receiver),
            ReceiveState::Wait => ReceiveResult::new(false),
        }
    }

    fn allocate_transport_message(&self, receiver: &MessageReceiver, peek: bool) -> ReceiveResult {
        let transport_address_pair = receiver.transport_address_pair();
        let mut message: Option<&'static TransportMessage> = None;
        if !receiver.is_blocked() {
            let mut first_frame = [0u8; MAX_FIRST_FRAME_DATA_SIZE];
            let size = receiver.first_frame_data(&mut first_frame);
            match self.message_providing_listener.get_transport_message(
                self.bus_id,
                transport_address_pair.source_id(),
                transport_address_pair.target_id(),
                receiver.message_size(),
                &first_frame[..size],
            ) {
                Ok(provided) => message = Some(provided),
                Err(ProviderErrorCode::NoMsgAvailable) => {}
                Err(error) => {
                    let mut buffer = [0u8; FORMAT_BUFFER_SIZE];
                    let address = self
                        .address_converter
                        .format_data_link_address(receiver.reception_address(), &mut buffer);
                    match error {
                        ProviderErrorCode::InvalidSrcAddress => log_warn!(
                            self.logger,
                            b"DoCanReceiver(%s)::allocateTransportMessage(%s): Illegal source id.",
                            Arg::Str(Some(self.name())),
                            Arg::Str(Some(address))
                        ),
                        ProviderErrorCode::InvalidTgtAddress => log_warn!(
                            self.logger,
                            b"DoCanReceiver(%s)::allocateTransportMessage(%s): Illegal target id.",
                            Arg::Str(Some(self.name())),
                            Arg::Str(Some(address))
                        ),
                        _ => log_warn!(
                            self.logger,
                            b"DoCanReceiver(%s)::allocateTransportMessage(%s): No buffer available (error: 0x%x). Message discarded.",
                            Arg::Str(Some(self.name())),
                            Arg::Str(Some(address)),
                            provider_error_code(error)
                        ),
                    }
                    return receiver.handler().cancel(ReceiveMessage::None);
                }
            }
            if let Some(message) = message {
                message.reset_valid_bytes();
                message.set_source_address(transport_address_pair.source_id());
                message.set_target_address(transport_address_pair.target_id());
                message.set_payload_length(receiver.message_size());
            }
        }
        if message.is_none() && peek {
            return ReceiveResult::new(false);
        }
        receiver.allocated(message, self.parameters.max_allocate_retry_count())
    }

    fn send_flow_control_frame(&self, receiver: &MessageReceiver) -> ReceiveResult {
        let success = if receiver.handler().is_flow_control_wait() {
            self.flow_control_frame_transmitter.send_flow_control(
                receiver.frame_codec(),
                receiver.transmission_address(),
                FlowStatus::Wait,
                0,
                0,
            )
        } else {
            self.flow_control_frame_transmitter.send_flow_control(
                receiver.frame_codec(),
                receiver.transmission_address(),
                FlowStatus::Cts,
                receiver.max_block_size(),
                receiver.encoded_min_separation_time(),
            )
        };
        receiver.handler().frame_sent(success)
    }

    fn start_processing_transport_message(&self, receiver: &MessageReceiver) -> ReceiveResult {
        let Some(message) = receiver.detach_message() else {
            return receiver.handler().processed(false);
        };
        let success = self.message_providing_listener.message_received(
            self.bus_id,
            message,
            Some(self.process_message_receivers.owner),
        ) == TransportReceiveResult::NoError;
        if !success {
            self.message_providing_listener.release_transport_message(message);
        }
        receiver.handler().processed(success)
    }

    fn release(&self, receiver: &MessageReceiver) -> ReceiveResult {
        let reception_address = receiver.reception_address();
        if let Some(message) = receiver.release() {
            self.message_providing_listener.release_transport_message(message);
        }
        for index in 0..self.pool.len() {
            let other = self.pool.at(index);
            if other.is_blocked() && other.reception_address() == reception_address {
                other.set_blocked(false);
                break;
            }
        }
        assert!(self.released_receiver_count.get() != u8::MAX, "receive count must not wrap");
        self.released_receiver_count.set(self.released_receiver_count.get() + 1);
        ReceiveResult::new(false)
    }

    fn reset_timer(&self, receiver: &MessageReceiver) {
        let now_us = self.parameters.now_us();
        match receiver.handler().timeout() {
            ReceiveTimeout::Allocate => {
                receiver.set_timer(
                    now_us.wrapping_add(u32::from(self.parameters.wait_allocate_timeout()) * 1000),
                );
            }
            ReceiveTimeout::Rx => receiver.set_timer(
                now_us.wrapping_add(u32::from(self.parameters.wait_rx_timeout()) * 1000),
            ),
            ReceiveTimeout::None => receiver.set_timer(now_us),
        }
    }

    fn handle_pending_message_receivers(&self, reception_address: Address) -> bool {
        let mut blocked = false;
        for index in 0..self.pool.len() {
            let receiver = self.pool.at(index);
            if receiver.reception_address() == reception_address {
                if receiver.handler().frame_count() > 1 {
                    let mut buffer = [0u8; FORMAT_BUFFER_SIZE];
                    let address = self
                        .address_converter
                        .format_data_link_address(receiver.reception_address(), &mut buffer);
                    log_info!(
                        self.logger,
                        b"DoCanReceiver(%s)::handlingPendingMessageReceivers(%s): Segmented transfer cancelled due to new first frame.",
                        Arg::Str(Some(self.name())),
                        Arg::Str(Some(address))
                    );
                    self.handle_transitions(
                        receiver,
                        receiver.handler().cancel(ReceiveMessage::None),
                        b"handlingPendingMessageReceivers",
                    );
                } else {
                    blocked = true;
                }
            }
        }
        blocked
    }

    fn find_message_receiver(
        &self,
        reception_address: Address,
    ) -> Option<&'static MessageReceiver> {
        (0..self.pool.len())
            .map(|index| self.pool.at(index))
            .find(|receiver| receiver.reception_address() == reception_address)
    }

    fn remove_guard(&self) -> RemoveGuard<'_, L> {
        self.set_remove_lock();
        RemoveGuard(self)
    }

    fn set_remove_lock(&self) {
        assert!(self.remove_lock_count.get() != u8::MAX, "lock count must not wrap");
        let _lock = L::lock();
        self.remove_lock_count.set(self.remove_lock_count.get() + 1);
    }

    fn release_remove_lock(&self) {
        assert!(self.remove_lock_count.get() != 0, "lock count must not wrap");
        let _lock = L::lock();
        self.remove_lock_count.set(self.remove_lock_count.get() - 1);
        if self.remove_lock_count.get() == 0 && self.released_receiver_count.get() > 0 {
            let mut index = 0;
            while self.released_receiver_count.get() > 0 && index < self.pool.len() {
                if self.pool.at(index).handler().state() == ReceiveState::Done {
                    self.pool.remove(index);
                    self.released_receiver_count.set(self.released_receiver_count.get() - 1);
                } else {
                    index += 1;
                }
            }
        }
    }

    fn name(&self) -> &'static [u8] {
        bus_name(self.bus_id)
    }
}

impl<L: Lock + 'static> TransportMessageProcessedListener for DoCanReceiver<L> {
    fn transport_message_processed(
        &self,
        message: &'static TransportMessage,
        _result: ProcessingResult,
    ) {
        self.message_providing_listener.release_transport_message(message);
        openbsw_async::execute(
            self.context,
            &self.process_message_receivers.owner.process_message_receivers,
        );
    }
}

/// The C++ enum value of a provider error, for the log.
fn provider_error_code(error: ProviderErrorCode) -> u8 {
    match error {
        ProviderErrorCode::InvalidSrcAddress => 1,
        ProviderErrorCode::InvalidTgtAddress => 2,
        ProviderErrorCode::NoMsgAvailable => 3,
        ProviderErrorCode::SizeTooLarge => 4,
        ProviderErrorCode::NotResponsible => 5,
    }
}

// Ported from docan/test/src/docan/receiver/DoCanMessageReceiveProtocolHandlerTest.cpp.
#[cfg(test)]
mod tests {
    use super::*;

    fn result(transition: bool) -> ReceiveResult {
        ReceiveResult::new(transition)
    }

    #[test]
    fn result_class() {
        let cut = ReceiveResult::new(true);
        assert!(cut.has_transition());
        assert_eq!(cut.message(), ReceiveMessage::None);
        assert_eq!(cut.param(), 0);
        let cut = cut.with_message(ReceiveMessage::IllegalState);
        assert_eq!(cut.message(), ReceiveMessage::IllegalState);
        let cut = cut.with_message_param(ReceiveMessage::BadSequenceNumber, 2);
        assert_eq!((cut.message(), cut.param()), (ReceiveMessage::BadSequenceNumber, 2));
        assert_eq!(
            result(true).with_message_param(ReceiveMessage::IllegalState, 0x7f),
            result(true).with_message_param(ReceiveMessage::IllegalState, 0x7f)
        );
        assert_ne!(
            result(false).with_message_param(ReceiveMessage::IllegalState, 0x7f),
            result(true).with_message_param(ReceiveMessage::IllegalState, 0x7f)
        );
        assert_ne!(
            result(true).with_message_param(ReceiveMessage::IllegalState, 0x7e),
            result(true).with_message_param(ReceiveMessage::IllegalState, 0x7f)
        );
    }

    #[test]
    fn state_after_construction_and_init() {
        let cut = ReceiveProtocolHandler::new(0);
        assert_eq!(cut.state(), ReceiveState::Allocate);
        assert_eq!(cut.timeout(), ReceiveTimeout::None);
        assert_eq!(cut.frame_index(), 1);
        assert_eq!(cut.frame_count(), 0);
        assert!(cut.is_allocating());
        cut.reset(1);
        assert_eq!(cut.frame_count(), 1);
    }

    #[test]
    fn state_processing_after_successful_allocation_then_done() {
        let cut = ReceiveProtocolHandler::new(1);
        assert_eq!(cut.allocated(true, 1), result(true));
        assert_eq!(cut.state(), ReceiveState::Processing);
        assert!(!cut.is_allocating());
        assert!(!cut.is_flow_control_wait());
        assert_eq!(cut.processed(true), result(true));
        assert_eq!(cut.state(), ReceiveState::Done);
        let cut = ReceiveProtocolHandler::new(1);
        cut.allocated(true, 1);
        assert_eq!(
            cut.processed(false),
            result(true).with_message(ReceiveMessage::ProcessingFailed)
        );
        assert_eq!(cut.state(), ReceiveState::Done);
    }

    #[test]
    fn state_receive_for_each_consecutive_frame() {
        let cut = ReceiveProtocolHandler::new(3);
        assert_eq!(cut.allocated(true, 1), result(true));
        assert_eq!(cut.frame_sent(true), result(true));
        assert_eq!(
            (cut.state(), cut.timeout(), cut.frame_index()),
            (ReceiveState::Wait, ReceiveTimeout::Rx, 1)
        );
        assert_eq!(cut.consecutive_frame_received(1, 0), result(true));
        assert_eq!(
            (cut.state(), cut.timeout(), cut.frame_index()),
            (ReceiveState::Wait, ReceiveTimeout::Rx, 2)
        );
        assert_eq!(cut.consecutive_frame_received(2, 0), result(true));
        assert_eq!(
            (cut.state(), cut.timeout(), cut.frame_index()),
            (ReceiveState::Processing, ReceiveTimeout::None, 3)
        );
    }

    #[test]
    fn state_send_after_end_of_block() {
        let cut = ReceiveProtocolHandler::new(4);
        assert_eq!(cut.allocated(true, 1), result(true));
        assert_eq!(cut.frame_sent(true), result(true));
        assert_eq!(cut.consecutive_frame_received(1, 2), result(true));
        assert_eq!((cut.state(), cut.timeout()), (ReceiveState::Wait, ReceiveTimeout::Rx));
        assert_eq!(cut.consecutive_frame_received(2, 2), result(true));
        assert_eq!((cut.state(), cut.timeout()), (ReceiveState::Send, ReceiveTimeout::None));
        assert_eq!(cut.frame_sent(true), result(true));
        assert_eq!(cut.frame_index(), 3);
        assert_eq!(cut.consecutive_frame_received(3, 0), result(true));
        assert_eq!((cut.state(), cut.frame_index()), (ReceiveState::Processing, 4));
    }

    #[test]
    fn state_done_after_bad_sequence_number() {
        let cut = ReceiveProtocolHandler::new(3);
        cut.allocated(true, 1);
        cut.frame_sent(true);
        assert_eq!(cut.consecutive_frame_received(1, 0), result(true));
        assert_eq!(
            cut.consecutive_frame_received(1, 0),
            result(true).with_message_param(ReceiveMessage::BadSequenceNumber, 1)
        );
        assert_eq!((cut.state(), cut.timeout()), (ReceiveState::Done, ReceiveTimeout::None));
        assert!(!cut.is_allocating());
    }

    #[test]
    fn state_done_after_segmented_allocation_timeout() {
        let cut = ReceiveProtocolHandler::new(3);
        assert_eq!(cut.allocated(false, 2), result(true));
        assert_eq!((cut.state(), cut.timeout()), (ReceiveState::Send, ReceiveTimeout::None));
        assert!(cut.is_allocating());
        assert!(cut.is_flow_control_wait());
        assert_eq!(cut.frame_sent(true), result(true));
        assert_eq!((cut.state(), cut.timeout()), (ReceiveState::Wait, ReceiveTimeout::Allocate));
        assert_eq!(cut.expired(), result(true));
        assert_eq!(cut.allocated(false, 2), result(true));
        assert_eq!(cut.frame_sent(true), result(true));
        assert_eq!(cut.expired(), result(true));
        assert_eq!(
            cut.allocated(false, 2),
            result(true).with_message(ReceiveMessage::AllocationRetryCountExceeded)
        );
        assert_eq!(cut.state(), ReceiveState::Done);
        assert!(!cut.is_allocating());
    }

    #[test]
    fn state_done_after_single_frame_allocation_timeout() {
        let cut = ReceiveProtocolHandler::new(1);
        assert_eq!(cut.allocated(false, 2), result(true));
        assert_eq!((cut.state(), cut.timeout()), (ReceiveState::Wait, ReceiveTimeout::Allocate));
        assert!(cut.is_allocating());
        assert!(!cut.is_flow_control_wait());
        assert_eq!(
            cut.allocated(false, 2),
            result(true).with_message(ReceiveMessage::AllocationRetryCountExceeded)
        );
        assert_eq!(cut.state(), ReceiveState::Done);
    }

    #[test]
    fn state_done_after_reception_timeout_and_cancel() {
        let cut = ReceiveProtocolHandler::new(4);
        cut.allocated(true, 1);
        cut.frame_sent(true);
        assert_eq!(cut.expired(), result(true).with_message(ReceiveMessage::RxTimeoutExpired));
        assert_eq!(cut.state(), ReceiveState::Done);
        let cut = ReceiveProtocolHandler::new(1);
        cut.allocated(true, 1);
        assert_eq!(
            cut.cancel(ReceiveMessage::IllegalState),
            result(true).with_message(ReceiveMessage::IllegalState)
        );
        assert_eq!(cut.state(), ReceiveState::Done);
    }

    #[test]
    fn shutdown_during_processing_and_allocation() {
        let cut = ReceiveProtocolHandler::new(1);
        cut.allocated(true, 1);
        assert_eq!(cut.shutdown(), result(false));
        let cut = ReceiveProtocolHandler::new(1);
        assert_eq!(cut.shutdown(), result(true));
    }

    #[test]
    fn illegal_and_ignored_calls() {
        let cut = ReceiveProtocolHandler::new(1);
        cut.allocated(true, 1);
        assert_eq!(
            cut.frame_sent(true),
            result(true)
                .with_message_param(ReceiveMessage::IllegalState, ReceiveState::Processing as u8)
        );
        assert_eq!(cut.state(), ReceiveState::Processing);
        assert_eq!(cut.consecutive_frame_received(1, 0), result(false));
        assert_eq!(
            cut.allocated(true, 1),
            result(true)
                .with_message_param(ReceiveMessage::IllegalState, ReceiveState::Processing as u8)
        );
        assert_eq!(cut.expired(), result(false));
        let cut = ReceiveProtocolHandler::new(1);
        assert_eq!(cut.allocated(false, 1), result(true));
        assert_eq!(cut.consecutive_frame_received(1, 0), result(false));
        assert_eq!((cut.state(), cut.timeout()), (ReceiveState::Wait, ReceiveTimeout::Allocate));
    }

    #[test]
    fn pool_orders_by_timer() {
        static POOL: ReceiverPool<3> = ReceiverPool::new();
        assert!(!POOL.is_full());
        let a = POOL.allocate().unwrap();
        let b = POOL.allocate().unwrap();
        let c = POOL.allocate().unwrap();
        assert!(POOL.is_full());
        assert!(POOL.allocate().is_none());
        b.set_timer(10);
        c.set_timer(5);
        POOL.sort();
        assert!(core::ptr::eq(POOL.at(0), c));
        assert!(core::ptr::eq(POOL.at(1), b));
        assert!(core::ptr::eq(POOL.at(2), a));
        POOL.remove(1);
        assert_eq!(POOL.len(), 2);
        assert!(core::ptr::eq(POOL.at(1), a));
        assert!(!b.is_used());
        let d = POOL.allocate().unwrap();
        assert!(core::ptr::eq(d, b));
    }
}
