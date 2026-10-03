// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The sending side, ported from `transmitter/DoCanMessageTransmitProtocolHandler.h`,
//! `DoCanMessageTransmitter.h` and `DoCanTransmitter.h`: one state machine per message
//! being sent, in a pool of slots, paced by flow control and the separation time.

use core::cell::Cell;
use core::marker::PhantomData;

use openbsw_async::{ContextType, QueueNode, Runnable};
use openbsw_timer::Lock;
use openbsw_transport::{
    LayerErrorCode, ProcessingResult, TransportMessage, TransportMessageProcessedListener, bus_name,
};
use openbsw_util::format::Arg;
use openbsw_util::log_warn;
use openbsw_util::logger::LoggerComponent;

use crate::addressing::AddressConverter;
use crate::codec::FrameCodec;
use crate::common::{
    Address, DataLinkAddressPair, DoCanParameters, FlowStatus, FrameIndex, FrameSize,
    INVALID_ADDRESS, JobHandle, MessageSize, TransportAddressPair, time_less,
};
use crate::transceiver::{
    DataFrameTransmitter, DataFrameTransmitterCallback, SendResult, TickGenerator,
};

const FORMAT_BUFFER_SIZE: usize = 32;

/// Where a transmission stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransmitState {
    /// Not started.
    Initialized = 0,
    /// A frame is due.
    Send = 1,
    /// Waiting for the driver, flow control or the separation time.
    Wait = 2,
    /// Sent.
    Success = 3,
    /// Given up.
    Fail = 4,
}

/// What a waiting transmission waits for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransmitTimeout {
    /// Nothing.
    None,
    /// The driver's transmit callback.
    TxCallback,
    /// The receiver's flow control.
    FlowControl,
    /// The separation time between consecutive frames.
    SeparationTime,
}

/// What a transition reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransmitMessage {
    /// Nothing to say.
    None,
    /// The call did not fit the state.
    IllegalState,
    /// The driver did not confirm in time.
    TxCallbackTimeoutExpired,
    /// Too many `WAIT` flow controls.
    FlowControlWaitCountExceeded,
    /// The receiver reported an overflow.
    FlowControlOverflow,
    /// No flow control came in time.
    FlowControlTimeoutExpired,
    /// A flow control frame was not understood.
    FlowControlInvalid,
}

/// What the owner must do after a transition (`TransmitAction`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TransmitActions {
    /// Remember the separation time the flow control carried.
    pub store_separation_time: bool,
    /// Cancel the frames queued with the driver.
    pub cancel_send: bool,
}

/// The outcome of a protocol handler call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransmitResult {
    transition: bool,
    actions: TransmitActions,
    message: TransmitMessage,
    param: u8,
}

impl TransmitResult {
    /// A result with or without a transition.
    pub const fn new(transition: bool) -> Self {
        Self {
            transition,
            actions: TransmitActions { store_separation_time: false, cancel_send: false },
            message: TransmitMessage::None,
            param: 0,
        }
    }

    /// With `actions`.
    pub const fn with_actions(self, actions: TransmitActions) -> Self {
        Self { actions, ..self }
    }

    /// With `message`.
    pub const fn with_message(self, message: TransmitMessage) -> Self {
        Self { message, param: 0, ..self }
    }

    /// With `message` and `param`.
    pub const fn with_message_param(self, message: TransmitMessage, param: u8) -> Self {
        Self { message, param, ..self }
    }

    /// Whether the state changed.
    pub const fn has_transition(&self) -> bool {
        self.transition
    }

    /// What to do.
    pub const fn actions(&self) -> TransmitActions {
        self.actions
    }

    /// The message.
    pub const fn message(&self) -> TransmitMessage {
        self.message
    }

    /// The message's parameter.
    pub const fn param(&self) -> u8 {
        self.param
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FlowControl {
    Unexpected,
    Expected,
    ReceivedCts,
    ReceivedWait,
}

/// The state machine of one transmission: the port of `DoCanMessageTransmitProtocolHandler`.
pub struct TransmitProtocolHandler {
    frame_index: Cell<FrameIndex>,
    frame_count: Cell<FrameIndex>,
    block_end: Cell<FrameIndex>,
    state: Cell<TransmitState>,
    timeout: Cell<TransmitTimeout>,
    flow_control: Cell<FlowControl>,
    error_message: Cell<TransmitMessage>,
    flow_control_wait_count: Cell<u8>,
    has_min_separation_time: Cell<bool>,
}

// SAFETY: driven under the transmitter's lock.
unsafe impl Sync for TransmitProtocolHandler {}

impl TransmitProtocolHandler {
    /// A handler for a message of `frame_count` frames.
    pub const fn new(frame_count: FrameIndex) -> Self {
        Self {
            frame_index: Cell::new(0),
            frame_count: Cell::new(frame_count),
            block_end: Cell::new(1),
            state: Cell::new(TransmitState::Initialized),
            timeout: Cell::new(TransmitTimeout::None),
            flow_control: Cell::new(FlowControl::Unexpected),
            error_message: Cell::new(TransmitMessage::None),
            flow_control_wait_count: Cell::new(0),
            has_min_separation_time: Cell::new(false),
        }
    }

    /// Start over for a message of `frame_count` frames.
    pub fn reset(&self, frame_count: FrameIndex) {
        self.frame_index.set(0);
        self.frame_count.set(frame_count);
        self.block_end.set(1);
        self.state.set(TransmitState::Initialized);
        self.timeout.set(TransmitTimeout::None);
        self.flow_control.set(FlowControl::Unexpected);
        self.error_message.set(TransmitMessage::None);
        self.flow_control_wait_count.set(0);
        self.has_min_separation_time.set(false);
    }

    /// The state.
    pub fn state(&self) -> TransmitState {
        self.state.get()
    }

    /// What is waited for.
    pub fn timeout(&self) -> TransmitTimeout {
        self.timeout.get()
    }

    /// The next frame's index.
    pub fn frame_index(&self) -> FrameIndex {
        self.frame_index.get()
    }

    /// The index the current block ends at.
    pub fn block_end(&self) -> FrameIndex {
        self.block_end.get()
    }

    /// The message's frame count.
    pub fn frame_count(&self) -> FrameIndex {
        self.frame_count.get()
    }

    /// Whether the transmission ended, either way.
    pub fn is_done(&self) -> bool {
        matches!(self.state.get(), TransmitState::Success | TransmitState::Fail)
    }

    /// The message of the failure, if any.
    pub fn error_message(&self) -> TransmitMessage {
        self.error_message.get()
    }

    /// Give up with `message`.
    pub fn cancel(&self, message: TransmitMessage) -> TransmitResult {
        self.set_failed(message, 0)
    }

    /// Begin.
    pub fn start(&self) -> TransmitResult {
        self.set_send()
    }

    /// A frame was handed to the driver.
    pub fn frame_sending(&self) -> TransmitResult {
        if self.state.get() == TransmitState::Send {
            let next_frame_index = self.frame_index.get() + 1;
            if next_frame_index == self.block_end.get()
                && next_frame_index != self.frame_count.get()
            {
                self.flow_control_wait_count.set(0);
                return self.set_state(
                    TransmitState::Wait,
                    FlowControl::Expected,
                    TransmitTimeout::TxCallback,
                );
            }
            return self.set_state(
                TransmitState::Wait,
                FlowControl::Unexpected,
                TransmitTimeout::TxCallback,
            );
        }
        self.set_failed(TransmitMessage::IllegalState, self.state.get() as u8)
    }

    /// The driver confirmed `frame_count` frames.
    pub fn frames_sent(&self, frame_count: FrameIndex) -> TransmitResult {
        if self.state.get() != TransmitState::Wait
            || self.timeout.get() != TransmitTimeout::TxCallback
        {
            return TransmitResult::new(true)
                .with_message_param(TransmitMessage::IllegalState, self.state.get() as u8);
        }
        self.frame_index.set(self.frame_index.get() + frame_count);
        if self.frame_index.get() >= self.frame_count.get() {
            return self.set_state(
                TransmitState::Success,
                FlowControl::Unexpected,
                TransmitTimeout::None,
            );
        }
        if frame_count > 1 && self.frame_index.get() >= self.block_end.get() {
            self.frame_index.set(self.block_end.get());
            self.flow_control.set(FlowControl::Expected);
        }
        match self.flow_control.get() {
            FlowControl::ReceivedCts => self.set_send(),
            FlowControl::ReceivedWait | FlowControl::Expected => self.set_state(
                TransmitState::Wait,
                self.flow_control.get(),
                TransmitTimeout::FlowControl,
            ),
            FlowControl::Unexpected => {
                if self.has_min_separation_time.get() {
                    self.set_state(
                        TransmitState::Wait,
                        FlowControl::Unexpected,
                        TransmitTimeout::SeparationTime,
                    )
                } else {
                    self.set_send()
                }
            }
        }
    }

    /// A flow control frame arrived.
    pub fn handle_flow_control(
        &self,
        flow_status: FlowStatus,
        block_size: u8,
        has_min_separation_time: bool,
        max_wait_count: u8,
    ) -> TransmitResult {
        if self.flow_control.get() != FlowControl::Expected {
            return TransmitResult::new(false);
        }
        match flow_status {
            FlowStatus::Cts => {
                self.block_end.set(if block_size > 0 {
                    self.block_end.get() + FrameIndex::from(block_size)
                } else {
                    self.frame_count.get()
                });
                self.has_min_separation_time.set(has_min_separation_time);
                let actions = TransmitActions { store_separation_time: true, cancel_send: false };
                if self.timeout.get() == TransmitTimeout::FlowControl {
                    let _ = self.set_send();
                    return TransmitResult::new(true).with_actions(actions);
                }
                self.flow_control.set(FlowControl::ReceivedCts);
                TransmitResult::new(false).with_actions(actions)
            }
            FlowStatus::Wait => {
                if self.flow_control_wait_count.get() < max_wait_count {
                    self.flow_control_wait_count.set(self.flow_control_wait_count.get() + 1);
                    if self.timeout.get() == TransmitTimeout::FlowControl {
                        return self.set_state(
                            TransmitState::Wait,
                            FlowControl::Expected,
                            TransmitTimeout::FlowControl,
                        );
                    }
                    self.flow_control.set(FlowControl::ReceivedWait);
                    return TransmitResult::new(false);
                }
                self.set_failed(TransmitMessage::FlowControlWaitCountExceeded, 0)
            }
            FlowStatus::Ovflw => self.set_failed(TransmitMessage::FlowControlOverflow, 0),
            FlowStatus::Invalid(_) => self.set_failed(TransmitMessage::FlowControlInvalid, 0),
        }
    }

    /// The timeout expired.
    pub fn expired(&self) -> TransmitResult {
        match self.timeout.get() {
            TransmitTimeout::TxCallback => {
                self.set_failed(TransmitMessage::TxCallbackTimeoutExpired, 0)
            }
            TransmitTimeout::FlowControl => {
                self.set_failed(TransmitMessage::FlowControlTimeoutExpired, 0)
            }
            TransmitTimeout::SeparationTime => self.set_send(),
            TransmitTimeout::None => TransmitResult::new(false),
        }
    }

    fn set_send(&self) -> TransmitResult {
        self.state.set(TransmitState::Send);
        self.flow_control.set(FlowControl::Unexpected);
        self.timeout.set(TransmitTimeout::TxCallback);
        TransmitResult::new(true)
    }

    fn set_state(
        &self,
        state: TransmitState,
        flow_control: FlowControl,
        timeout: TransmitTimeout,
    ) -> TransmitResult {
        self.state.set(state);
        self.flow_control.set(flow_control);
        self.timeout.set(timeout);
        TransmitResult::new(true)
    }

    fn set_failed(&self, message: TransmitMessage, param: u8) -> TransmitResult {
        let cancel_send = self.state.get() == TransmitState::Wait
            && self.timeout.get() == TransmitTimeout::TxCallback;
        self.state.set(TransmitState::Fail);
        self.flow_control.set(FlowControl::Unexpected);
        self.timeout.set(TransmitTimeout::None);
        self.error_message.set(message);
        TransmitResult::new(true)
            .with_message_param(message, param)
            .with_actions(TransmitActions { store_separation_time: false, cancel_send })
    }
}

/// One transmission: the handler plus the message and the timer (`DoCanMessageTransmitter`),
/// in a slot of a [`TransmitterPool`].
pub struct MessageTransmitter {
    handler: TransmitProtocolHandler,
    used: Cell<bool>,
    removed: Cell<bool>,
    codec: Cell<Option<&'static FrameCodec>>,
    message: Cell<Option<&'static TransportMessage>>,
    notification_listener: Cell<Option<&'static dyn TransportMessageProcessedListener>>,
    reception_address: Cell<Address>,
    transmission_address: Cell<Address>,
    min_separation_time_us: Cell<u32>,
    timer: Cell<u32>,
    job_handle: Cell<JobHandle>,
    bytes_sent: Cell<MessageSize>,
    consecutive_frame_data_size: Cell<FrameSize>,
    is_timer_set: Cell<bool>,
    is_sending_consecutive_frames: Cell<bool>,
}

// SAFETY: driven under the transmitter's lock.
unsafe impl Sync for MessageTransmitter {}

impl MessageTransmitter {
    /// An unused slot.
    pub const fn new() -> Self {
        Self {
            handler: TransmitProtocolHandler::new(0),
            used: Cell::new(false),
            removed: Cell::new(false),
            codec: Cell::new(None),
            message: Cell::new(None),
            notification_listener: Cell::new(None),
            reception_address: Cell::new(INVALID_ADDRESS),
            transmission_address: Cell::new(INVALID_ADDRESS),
            min_separation_time_us: Cell::new(0),
            timer: Cell::new(0),
            job_handle: Cell::new(JobHandle::new(0, 0)),
            bytes_sent: Cell::new(0),
            consecutive_frame_data_size: Cell::new(0),
            is_timer_set: Cell::new(false),
            is_sending_consecutive_frames: Cell::new(false),
        }
    }

    /// Take the slot for a new transmission (the C++ constructor).
    #[expect(clippy::too_many_arguments, reason = "the C++ constructor's parameters")]
    pub fn start(
        &self,
        job_handle: JobHandle,
        codec: &'static FrameCodec,
        data_link_address_pair: DataLinkAddressPair,
        message: &'static TransportMessage,
        notification_listener: Option<&'static dyn TransportMessageProcessedListener>,
        frame_count: FrameIndex,
        consecutive_frame_data_size: FrameSize,
    ) {
        self.handler.reset(frame_count);
        self.used.set(true);
        self.removed.set(false);
        self.codec.set(Some(codec));
        self.message.set(Some(message));
        self.notification_listener.set(notification_listener);
        self.reception_address.set(data_link_address_pair.reception_address());
        self.transmission_address.set(data_link_address_pair.transmission_address());
        self.min_separation_time_us.set(0);
        self.timer.set(0);
        self.job_handle.set(job_handle);
        self.bytes_sent.set(0);
        self.consecutive_frame_data_size.set(consecutive_frame_data_size);
        self.is_timer_set.set(false);
        self.is_sending_consecutive_frames.set(false);
    }

    /// The state machine.
    pub fn handler(&self) -> &TransmitProtocolHandler {
        &self.handler
    }

    /// Whether the slot holds a transmission.
    pub fn is_used(&self) -> bool {
        self.used.get()
    }

    /// The job handle.
    pub fn job_handle(&self) -> JobHandle {
        self.job_handle.get()
    }

    /// The message.
    pub fn message(&self) -> Option<&'static TransportMessage> {
        self.message.get()
    }

    /// Who to tell when done.
    pub fn notification_listener(&self) -> Option<&'static dyn TransportMessageProcessedListener> {
        self.notification_listener.get()
    }

    /// The CAN id flow control arrives on.
    pub fn reception_address(&self) -> Address {
        self.reception_address.get()
    }

    /// The CAN id frames go to.
    pub fn transmission_address(&self) -> Address {
        self.transmission_address.get()
    }

    /// How many bytes are still to send.
    pub fn send_data_offset(&self) -> MessageSize {
        self.bytes_sent.get()
    }

    /// The data bytes per consecutive frame.
    pub fn consecutive_frame_data_size(&self) -> FrameSize {
        self.consecutive_frame_data_size.get()
    }

    /// The separation time the receiver asked for.
    pub fn min_separation_time_us(&self) -> u32 {
        self.min_separation_time_us.get()
    }

    /// Whether the separation timer is running.
    pub fn is_sending_consecutive_frames(&self) -> bool {
        self.is_sending_consecutive_frames.get()
    }

    /// The codec.
    pub fn frame_codec(&self) -> &'static FrameCodec {
        self.codec.get().expect("a used transmitter has a codec")
    }

    /// A flow control frame arrived; a `CTS` stores its separation time.
    pub fn handle_flow_control(
        &self,
        flow_status: FlowStatus,
        block_size: u8,
        min_separation_time_us: u32,
        max_wait_count: u8,
    ) -> TransmitResult {
        let result = self.handler.handle_flow_control(
            flow_status,
            block_size,
            min_separation_time_us > 0,
            max_wait_count,
        );
        if result.actions().store_separation_time {
            self.min_separation_time_us.set(min_separation_time_us);
        }
        result
    }

    /// The driver confirmed `frame_count` frames with `data_size` bytes.
    pub fn frames_sent(&self, frame_count: FrameIndex, data_size: MessageSize) -> TransmitResult {
        self.bytes_sent.set(self.bytes_sent.get().wrapping_add(data_size));
        self.handler.frames_sent(frame_count)
    }

    /// Forget the job and the addresses.
    pub fn release(&self) {
        self.job_handle.set(JobHandle::default());
        self.reception_address.set(INVALID_ADDRESS);
        self.transmission_address.set(INVALID_ADDRESS);
    }

    /// Whether the timer is at or past `now_us`.
    pub fn timer_expired(&self, now_us: u32) -> bool {
        time_less(self.timer.get(), now_us.wrapping_add(1))
    }

    /// Arm the timer for `next_expiry_us`; `is_consecutive_frames` marks the separation time.
    pub fn set_timer(&self, next_expiry_us: u32, is_consecutive_frames: bool) {
        self.timer.set(next_expiry_us);
        self.is_timer_set.set(true);
        self.is_sending_consecutive_frames.set(is_consecutive_frames);
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

    fn sorts_before(&self, other: &MessageTransmitter) -> bool {
        if !self.is_timer_set.get() {
            return false;
        }
        if !other.is_timer_set.get() {
            return true;
        }
        time_less(self.timer.get(), other.timer.get())
    }
}

impl Default for MessageTransmitter {
    fn default() -> Self {
        Self::new()
    }
}

/// The transmissions a layer can hold at once: `N` slots and their order.
pub struct TransmitterPool<const N: usize> {
    slots: [MessageTransmitter; N],
    order: [Cell<u8>; N],
    count: Cell<usize>,
}

// SAFETY: changed under the transmitter's lock.
unsafe impl<const N: usize> Sync for TransmitterPool<N> {}

impl<const N: usize> TransmitterPool<N> {
    /// An empty pool.
    pub const fn new() -> Self {
        assert!(N <= 255, "a transmitter pool holds at most 255 slots");
        Self {
            slots: [const { MessageTransmitter::new() }; N],
            order: [const { Cell::new(0) }; N],
            count: Cell::new(0),
        }
    }
}

impl<const N: usize> Default for TransmitterPool<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// What the transmitter asks of its pool, whatever its size.
#[expect(clippy::len_without_is_empty, reason = "`is_full` is the question the pool answers")]
pub trait TransmitterSlots: Sync {
    /// Whether no slot is free.
    fn is_full(&self) -> bool;
    /// Take a free slot and put it at the end of the list; returns it and its slot index.
    fn allocate(&self) -> Option<(&'static MessageTransmitter, u16)>;
    /// How many transmitters are listed.
    fn len(&self) -> usize;
    /// The transmitter at `index` of the list.
    fn at(&self, index: usize) -> &'static MessageTransmitter;
    /// Take the transmitter at `index` out of the list (it stays allocated until `free`).
    fn remove(&self, index: usize);
    /// Free a transmitter taken out of the list.
    fn free(&self, transmitter: &MessageTransmitter);
    /// Every allocated transmitter that is out of the list.
    fn for_each_removed(&self, f: &mut dyn FnMut(&'static MessageTransmitter));
    /// Order the list by timer.
    fn sort(&self);
}

impl<const N: usize> TransmitterSlots for TransmitterPool<N> {
    fn is_full(&self) -> bool {
        self.count.get() == N
    }

    fn allocate(&self) -> Option<(&'static MessageTransmitter, u16)> {
        let (index, slot) = self.slots.iter().enumerate().find(|(_, slot)| !slot.is_used())?;
        slot.used.set(true);
        slot.removed.set(false);
        let count = self.count.get();
        self.order[count].set(index as u8);
        self.count.set(count + 1);
        // SAFETY: a pool is a `static` (its layer config is), so its slots live forever.
        Some((unsafe { &*(slot as *const MessageTransmitter) }, index as u16))
    }

    fn len(&self) -> usize {
        self.count.get()
    }

    fn at(&self, index: usize) -> &'static MessageTransmitter {
        let slot = &self.slots[usize::from(self.order[index].get())];
        // SAFETY: as in `allocate`.
        unsafe { &*(slot as *const MessageTransmitter) }
    }

    fn remove(&self, index: usize) {
        let count = self.count.get();
        self.slots[usize::from(self.order[index].get())].removed.set(true);
        for position in index..count - 1 {
            self.order[position].set(self.order[position + 1].get());
        }
        self.count.set(count - 1);
    }

    fn free(&self, transmitter: &MessageTransmitter) {
        transmitter.used.set(false);
        transmitter.removed.set(false);
    }

    fn for_each_removed(&self, f: &mut dyn FnMut(&'static MessageTransmitter)) {
        for slot in &self.slots {
            if slot.is_used() && slot.removed.get() {
                // SAFETY: as in `allocate`.
                f(unsafe { &*(slot as *const MessageTransmitter) });
            }
        }
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

/// The runnable that starts queued transmissions on the DoCAN context.
struct ProcessTransmitters<L: Lock + 'static> {
    owner: &'static DoCanTransmitter<L>,
    node: QueueNode<dyn Runnable>,
}

impl<L: Lock + 'static> Runnable for ProcessTransmitters<L> {
    fn execute(&self) {
        self.owner.process_message_transmitters();
    }

    fn node(&self) -> &QueueNode<dyn Runnable> {
        &self.node
    }
}

/// Sends messages on one bus: the port of `DoCanTransmitter`.
pub struct DoCanTransmitter<L: Lock + 'static> {
    address_converter: &'static dyn AddressConverter,
    pool: &'static dyn TransmitterSlots,
    process_message_transmitters: ProcessTransmitters<L>,
    data_frame_transmitter: &'static dyn DataFrameTransmitter,
    tick_generator: &'static dyn TickGenerator,
    /// The list position the next send starts looking from; `len()` is the C++ `end()`.
    send_message_transmitter: Cell<usize>,
    parameters: &'static DoCanParameters,
    job_counter: Cell<u16>,
    context: ContextType,
    bus_id: u8,
    logger: &'static LoggerComponent,
    remove_lock_count: Cell<u8>,
    released_transmitter_count: Cell<u8>,
    sending_consecutive_frames_count: Cell<u8>,
    send_lock: Cell<bool>,
    pending_send: Cell<bool>,
    switch_context: Cell<bool>,
    timers_updated: Cell<bool>,
    _lock: PhantomData<fn() -> L>,
}

// SAFETY: the counters and flags change under `L` or on the transmitter's context.
unsafe impl<L: Lock + 'static> Sync for DoCanTransmitter<L> {}

struct RemoveGuard<'a, L: Lock + 'static> {
    transmitter: &'a DoCanTransmitter<L>,
    remove: bool,
}

impl<L: Lock + 'static> Drop for RemoveGuard<'_, L> {
    fn drop(&mut self) {
        self.transmitter.release_remove_lock(self.remove);
    }
}

impl<L: Lock + 'static> DoCanTransmitter<L> {
    /// A transmitter on `bus_id`, running on `context`, given its own `'static` address.
    #[expect(clippy::too_many_arguments, reason = "the C++ constructor's parameters")]
    pub const fn new(
        this: &'static DoCanTransmitter<L>,
        bus_id: u8,
        context: ContextType,
        data_frame_transmitter: &'static dyn DataFrameTransmitter,
        tick_generator: &'static dyn TickGenerator,
        pool: &'static dyn TransmitterSlots,
        address_converter: &'static dyn AddressConverter,
        parameters: &'static DoCanParameters,
        logger: &'static LoggerComponent,
    ) -> Self {
        Self {
            address_converter,
            pool,
            process_message_transmitters: ProcessTransmitters {
                owner: this,
                node: QueueNode::new(),
            },
            data_frame_transmitter,
            tick_generator,
            send_message_transmitter: Cell::new(0),
            parameters,
            job_counter: Cell::new(0),
            context,
            bus_id,
            logger,
            remove_lock_count: Cell::new(0),
            released_transmitter_count: Cell::new(0),
            sending_consecutive_frames_count: Cell::new(0),
            send_lock: Cell::new(false),
            pending_send: Cell::new(false),
            switch_context: Cell::new(false),
            timers_updated: Cell::new(false),
            _lock: PhantomData,
        }
    }

    /// Start counting jobs from zero.
    pub fn init(&self) {
        self.job_counter.set(0);
        self.send_message_transmitter.set(self.pool.len());
    }

    /// Give every transmission up.
    pub fn shutdown(&self) {
        let _guard = self.remove_guard(true);
        for index in 0..self.pool.len() {
            let transmitter = self.pool.at(index);
            let _lock = L::lock();
            self.handle_result(
                transmitter,
                transmitter.handler().cancel(TransmitMessage::None),
                b"shutdown",
            );
        }
    }

    /// Queue `message`; `notification_listener` is told when it was sent or failed.
    pub fn send(
        &self,
        message: &'static TransportMessage,
        notification_listener: Option<&'static dyn TransportMessageProcessedListener>,
    ) -> LayerErrorCode {
        let transport_address_pair =
            TransportAddressPair::new(message.source_id(), message.target_id());
        let Some((codec, data_link_address_pair)) =
            self.address_converter.transmission_parameters(&transport_address_pair)
        else {
            log_warn!(
                self.logger,
                b"DoCanTransmitter(%s)::send(0x%x -> 0x%x): invalid source/target pair.",
                Arg::Str(Some(self.name())),
                message.source_id(),
                message.target_id()
            );
            return LayerErrorCode::SendFail;
        };
        if !message.is_complete() {
            return LayerErrorCode::MessageIncomplete;
        }
        let Ok((frame_count, consecutive_frame_data_size)) =
            codec.encoded_frame_count(message.payload_length())
        else {
            return LayerErrorCode::GeneralError;
        };
        let _lock = L::lock();
        if frame_count > 1
            && self.find_by_reception_address(data_link_address_pair.reception_address()).is_some()
        {
            log_warn!(
                self.logger,
                b"DoCanTransmitter(%s)::send(0x%x -> 0x%x): Already a segmented message for this source/target pair!",
                Arg::Str(Some(self.name())),
                message.source_id(),
                message.target_id()
            );
            return LayerErrorCode::SendFail;
        }
        let Some((transmitter, slot)) =
            (if self.pool.is_full() { None } else { self.pool.allocate() })
        else {
            let mut buffer = [0u8; FORMAT_BUFFER_SIZE];
            let address = self
                .address_converter
                .format_data_link_address(data_link_address_pair.reception_address(), &mut buffer);
            log_warn!(
                self.logger,
                b"DoCanTransmitter(%s)::send(%s): No empty message transmitter found!",
                Arg::Str(Some(self.name())),
                Arg::Str(Some(address))
            );
            return LayerErrorCode::QueueFull;
        };
        self.job_counter.set(self.job_counter.get().wrapping_add(1));
        let job_handle = JobHandle::new(self.job_counter.get(), slot);
        transmitter.start(
            job_handle,
            codec,
            data_link_address_pair,
            message,
            notification_listener,
            frame_count,
            consecutive_frame_data_size,
        );
        openbsw_async::execute(
            self.context,
            &self.process_message_transmitters.owner.process_message_transmitters,
        );
        LayerErrorCode::Ok
    }

    /// A flow control frame arrived on `reception_address`.
    pub fn flow_control_frame_received(
        &self,
        reception_address: Address,
        flow_status: FlowStatus,
        block_size: u8,
        encoded_min_separation_time: u8,
    ) {
        let _guard = self.remove_guard(true);
        let _lock = L::lock();
        let Some(index) = self.find_by_reception_address(reception_address) else {
            let mut buffer = [0u8; FORMAT_BUFFER_SIZE];
            let address =
                self.address_converter.format_data_link_address(reception_address, &mut buffer);
            log_warn!(
                self.logger,
                b"DoCanTransmitter(%s)::flowControlFrameReceived(%s): no message pending to be sent!",
                Arg::Str(Some(self.name())),
                Arg::Str(Some(address))
            );
            return;
        };
        let transmitter = self.pool.at(index);
        self.handle_result(
            transmitter,
            transmitter.handle_flow_control(
                flow_status,
                block_size,
                DoCanParameters::decode_min_separation_time(encoded_min_separation_time),
                self.parameters.max_flow_control_wait_count(),
            ),
            b"flowControlFrameReceived",
        );
    }

    /// Whether a separation timer is running somewhere.
    pub fn is_sending_consecutive_frames(&self) -> bool {
        self.sending_consecutive_frames_count.get() > 0
    }

    /// Expire the timers that are due at `now_us`.
    pub fn cyclic_task(&self, now_us: u32) {
        {
            let _guard = self.remove_guard(true);
            for index in 0..self.pool.len() {
                let transmitter = self.pool.at(index);
                let _lock = L::lock();
                if !transmitter.update_timer(now_us) {
                    break;
                }
                self.handle_result(transmitter, transmitter.handler().expired(), b"cyclicTask");
            }
        }
        if self.timers_updated.get() {
            let _lock = L::lock();
            self.pool.sort();
            self.timers_updated.set(false);
        }
    }

    fn process_message_transmitters(&self) {
        let _guard = self.remove_guard(true);
        for index in 0..self.pool.len() {
            let transmitter = self.pool.at(index);
            if transmitter.handler().state() == TransmitState::Initialized {
                let _lock = L::lock();
                self.handle_result(
                    transmitter,
                    transmitter.handler().start(),
                    b"processMessageTransmitters",
                );
            }
        }
    }

    fn send_next_frames(&self) {
        loop {
            if let Some(index) = self.set_send_lock() {
                let transmitter = self.pool.at(index);
                let block_end = if transmitter.min_separation_time_us() == 0 {
                    transmitter.handler().block_end()
                } else {
                    transmitter.handler().frame_index() + 1
                };
                let message = transmitter.message().expect("a used transmitter has a message");
                let payload = message.payload();
                let offset = usize::from(transmitter.send_data_offset()).min(payload.len());
                let data = &payload[offset..];
                let result = self.data_frame_transmitter.start_send_data_frames(
                    transmitter.frame_codec(),
                    self.process_message_transmitters.owner,
                    transmitter.job_handle(),
                    transmitter.transmission_address(),
                    transmitter.handler().frame_index(),
                    block_end,
                    transmitter.consecutive_frame_data_size(),
                    data,
                );
                if matches!(result, SendResult::Queued | SendResult::QueuedFull) {
                    self.pending_send.set(result == SendResult::QueuedFull);
                    self.handle_result(
                        transmitter,
                        transmitter.handler().frame_sending(),
                        b"sendNextFrame",
                    );
                    self.release_send_lock(true);
                    continue;
                }
                if result == SendResult::Failed {
                    self.handle_result(
                        transmitter,
                        transmitter.handler().cancel(TransmitMessage::None),
                        b"sendNextFrames",
                    );
                }
            }
            self.release_send_lock(false);
            break;
        }
    }

    fn handle_result(
        &self,
        transmitter: &'static MessageTransmitter,
        result: TransmitResult,
        function_name: &[u8],
    ) {
        if result.has_transition() {
            self.reset_timer(transmitter);
            if transmitter.handler().is_done() {
                if result.actions().cancel_send {
                    self.data_frame_transmitter.cancel_send_data_frames(
                        self.process_message_transmitters.owner,
                        transmitter.job_handle(),
                    );
                    self.pending_send.set(false);
                }
                transmitter.release();
                assert!(
                    self.released_transmitter_count.get() != u8::MAX,
                    "transmitter count must not wrap"
                );
                self.released_transmitter_count.set(self.released_transmitter_count.get() + 1);
                self.switch_context.set(true);
            } else {
                self.timers_updated.set(true);
            }
        }
        if result.message() == TransmitMessage::None {
            return;
        }
        let (source, target) = transmitter
            .message()
            .map_or((0, 0), |message| (message.source_id(), message.target_id()));
        let name = Arg::Str(Some(self.name()));
        let function = Arg::Str(Some(function_name));
        match result.message() {
            TransmitMessage::IllegalState => log_warn!(
                self.logger,
                b"DoCanTransmitter(%s)::%s(0x%x -> 0x%x): Illegal state 0x%x!",
                name,
                function,
                source,
                target,
                result.param()
            ),
            TransmitMessage::TxCallbackTimeoutExpired => {
                log_warn!(
                    self.logger,
                    b"DoCanTransmitter(%s)::%s(0x%x -> 0x%x): Tx callback timeout",
                    name,
                    function,
                    source,
                    target
                )
            }
            TransmitMessage::FlowControlTimeoutExpired => {
                log_warn!(
                    self.logger,
                    b"DoCanTransmitter(%s)::%s(0x%x -> 0x%x): Flow control timeout",
                    name,
                    function,
                    source,
                    target
                )
            }
            TransmitMessage::FlowControlInvalid => log_warn!(
                self.logger,
                b"DoCanTransmitter(%s)::%s(0x%x -> 0x%x): Invalid flow control received",
                name,
                function,
                source,
                target
            ),
            TransmitMessage::FlowControlOverflow => log_warn!(
                self.logger,
                b"DoCanTransmitter(%s)::%s(0x%x -> 0x%x): Flow control overflow received",
                name,
                function,
                source,
                target
            ),
            TransmitMessage::FlowControlWaitCountExceeded => log_warn!(
                self.logger,
                b"DoCanTransmitter(%s)::%s(0x%x -> 0x%x): Flow control wait count exceeded",
                name,
                function,
                source,
                target
            ),
            TransmitMessage::None => {}
        }
    }

    fn reset_timer(&self, transmitter: &MessageTransmitter) {
        let now_us = self.parameters.now_us();
        if transmitter.is_sending_consecutive_frames() {
            assert!(self.sending_consecutive_frames_count.get() != 0, "frame count must not wrap");
            self.sending_consecutive_frames_count
                .set(self.sending_consecutive_frames_count.get() - 1);
        }
        match transmitter.handler().timeout() {
            TransmitTimeout::TxCallback => transmitter.set_timer(
                now_us.wrapping_add(u32::from(self.parameters.wait_tx_callback_timeout()) * 1000),
                false,
            ),
            TransmitTimeout::FlowControl => transmitter.set_timer(
                now_us.wrapping_add(u32::from(self.parameters.wait_flow_control_timeout()) * 1000),
                false,
            ),
            TransmitTimeout::SeparationTime => transmitter
                .set_timer(now_us.wrapping_add(transmitter.min_separation_time_us()), true),
            TransmitTimeout::None => transmitter.set_timer(now_us, false),
        }
        if transmitter.is_sending_consecutive_frames() {
            assert!(
                self.sending_consecutive_frames_count.get() != u8::MAX,
                "frame count must not wrap"
            );
            self.sending_consecutive_frames_count
                .set(self.sending_consecutive_frames_count.get() + 1);
        }
    }

    fn find_by_reception_address(&self, reception_address: Address) -> Option<usize> {
        (0..self.pool.len())
            .find(|&index| self.pool.at(index).reception_address() == reception_address)
    }

    fn find_by_job_handle(&self, job_handle: JobHandle) -> Option<usize> {
        (0..self.pool.len()).find(|&index| self.pool.at(index).job_handle() == job_handle)
    }

    fn set_send_lock(&self) -> Option<usize> {
        let _lock = L::lock();
        let len = self.pool.len();
        if self.send_lock.get() || self.pending_send.get() || len == 0 {
            return None;
        }
        if self.send_message_transmitter.get() >= len {
            self.send_message_transmitter.set(0);
        }
        let start = self.send_message_transmitter.get();
        loop {
            let index = self.send_message_transmitter.get();
            if self.pool.at(index).handler().state() == TransmitState::Send {
                self.send_lock.set(true);
                return Some(index);
            }
            let next = if index + 1 >= len { 0 } else { index + 1 };
            self.send_message_transmitter.set(next);
            if next == start {
                break;
            }
        }
        None
    }

    fn release_send_lock(&self, success: bool) {
        let _lock = L::lock();
        if success && self.send_message_transmitter.get() < self.pool.len() {
            self.send_message_transmitter.set(self.send_message_transmitter.get() + 1);
        }
        self.send_lock.set(false);
    }

    fn remove_guard(&self, remove: bool) -> RemoveGuard<'_, L> {
        self.set_remove_lock();
        RemoveGuard { transmitter: self, remove }
    }

    fn set_remove_lock(&self) {
        assert!(self.remove_lock_count.get() != u8::MAX, "lock count must not wrap");
        let _lock = L::lock();
        self.remove_lock_count.set(self.remove_lock_count.get() + 1);
    }

    fn release_remove_lock(&self, remove: bool) {
        self.send_next_frames();
        let switch_context;
        {
            assert!(self.remove_lock_count.get() != 0, "lock count must not wrap");
            let _lock = L::lock();
            self.remove_lock_count.set(self.remove_lock_count.get() - 1);
            if remove && self.remove_lock_count.get() == 0 {
                let mut index = 0;
                while self.released_transmitter_count.get() > 0 && index < self.pool.len() {
                    if self.pool.at(index).handler().is_done() {
                        if index == self.send_message_transmitter.get() {
                            self.pending_send.set(false);
                            self.send_message_transmitter.set(index + 1);
                        } else if index < self.send_message_transmitter.get() {
                            self.send_message_transmitter
                                .set(self.send_message_transmitter.get() - 1);
                        }
                        self.pool.remove(index);
                        self.released_transmitter_count
                            .set(self.released_transmitter_count.get() - 1);
                    } else {
                        index += 1;
                    }
                }
            }
            switch_context = self.switch_context.replace(false);
        }
        self.pool.for_each_removed(&mut |transmitter| {
            if let (Some(listener), Some(message)) =
                (transmitter.notification_listener(), transmitter.message())
            {
                let processing_result = if transmitter.handler().state() == TransmitState::Success {
                    ProcessingResult::NoError
                } else {
                    match transmitter.handler().error_message() {
                        TransmitMessage::FlowControlTimeoutExpired => {
                            ProcessingResult::ErrorTimeout
                        }
                        TransmitMessage::FlowControlOverflow => ProcessingResult::ErrorOverflow,
                        TransmitMessage::FlowControlInvalid => ProcessingResult::ErrorAbort,
                        _ => ProcessingResult::ErrorGeneral,
                    }
                };
                listener.transport_message_processed(message, processing_result);
            }
            let _lock = L::lock();
            self.pool.free(transmitter);
        });
        if switch_context {
            openbsw_async::execute(
                self.context,
                &self.process_message_transmitters.owner.process_message_transmitters,
            );
        }
    }

    fn name(&self) -> &'static [u8] {
        bus_name(self.bus_id)
    }
}

impl<L: Lock + 'static> DataFrameTransmitterCallback for DoCanTransmitter<L> {
    fn data_frames_sent(
        &self,
        job_handle: JobHandle,
        frame_count: FrameIndex,
        data_size: MessageSize,
    ) {
        let _guard = self.remove_guard(false);
        let _lock = L::lock();
        self.pending_send.set(false);
        if let Some(index) = self.find_by_job_handle(job_handle) {
            let transmitter = self.pool.at(index);
            self.handle_result(
                transmitter,
                transmitter.frames_sent(frame_count, data_size),
                b"dataFramesSent",
            );
            if self.is_sending_consecutive_frames() {
                self.tick_generator.tick_needed();
            }
        }
    }
}

// Ported from docan/test/src/docan/transmitter/DoCanMessageTransmitProtocolHandlerTest.cpp.
#[cfg(test)]
mod tests {
    use super::*;

    const STORE: TransmitActions =
        TransmitActions { store_separation_time: true, cancel_send: false };
    const CANCEL: TransmitActions =
        TransmitActions { store_separation_time: false, cancel_send: true };

    fn result(transition: bool) -> TransmitResult {
        TransmitResult::new(transition)
    }

    #[test]
    fn result_class() {
        let cut = TransmitResult::new(false);
        assert!(!cut.has_transition());
        assert_eq!(cut.actions(), TransmitActions::default());
        assert_eq!(cut.message(), TransmitMessage::None);
        let cut = cut.with_actions(STORE).with_message_param(TransmitMessage::IllegalState, 2);
        assert_eq!(cut.actions(), STORE);
        assert_eq!((cut.message(), cut.param()), (TransmitMessage::IllegalState, 2));
        let cut = cut.with_message(TransmitMessage::FlowControlTimeoutExpired);
        assert_eq!((cut.message(), cut.param()), (TransmitMessage::FlowControlTimeoutExpired, 0));
        assert_eq!(
            result(true)
                .with_actions(STORE)
                .with_message_param(TransmitMessage::IllegalState, 0x7f),
            result(true)
                .with_actions(STORE)
                .with_message_param(TransmitMessage::IllegalState, 0x7f)
        );
        assert_ne!(result(false).with_actions(STORE), result(true).with_actions(STORE));
        assert_ne!(result(true), result(true).with_actions(STORE));
    }

    #[test]
    fn state_after_construction_start_sending_and_sent() {
        let cut = TransmitProtocolHandler::new(22);
        assert_eq!(
            (cut.state(), cut.timeout(), cut.frame_index(), cut.block_end(), cut.frame_count()),
            (TransmitState::Initialized, TransmitTimeout::None, 0, 1, 22)
        );
        let cut = TransmitProtocolHandler::new(1);
        assert_eq!(cut.start(), result(true));
        assert_eq!(
            (cut.state(), cut.timeout()),
            (TransmitState::Send, TransmitTimeout::TxCallback)
        );
        assert_eq!(cut.frame_sending(), result(true));
        assert_eq!(
            (cut.state(), cut.timeout()),
            (TransmitState::Wait, TransmitTimeout::TxCallback)
        );
        assert_eq!(cut.frames_sent(1), result(true));
        assert!(cut.is_done());
        assert_eq!((cut.state(), cut.timeout()), (TransmitState::Success, TransmitTimeout::None));
    }

    #[test]
    fn state_done_after_cancel() {
        let cut = TransmitProtocolHandler::new(1);
        cut.start();
        cut.frame_sending();
        let result = cut.cancel(TransmitMessage::TxCallbackTimeoutExpired);
        assert_eq!(
            result,
            TransmitResult::new(true)
                .with_message(TransmitMessage::TxCallbackTimeoutExpired)
                .with_actions(CANCEL)
        );
        assert!(cut.is_done());
        assert_eq!(cut.state(), TransmitState::Fail);
        let cut = TransmitProtocolHandler::new(1);
        assert_eq!(
            cut.cancel(TransmitMessage::TxCallbackTimeoutExpired),
            TransmitResult::new(true).with_message(TransmitMessage::TxCallbackTimeoutExpired)
        );
        assert!(cut.is_done());
    }

    #[test]
    fn illegal_states() {
        let cut = TransmitProtocolHandler::new(1);
        cut.start();
        assert_eq!(
            cut.frames_sent(1),
            result(true)
                .with_message_param(TransmitMessage::IllegalState, TransmitState::Send as u8)
        );
        assert_eq!(
            (cut.state(), cut.timeout()),
            (TransmitState::Send, TransmitTimeout::TxCallback)
        );
        let cut = TransmitProtocolHandler::new(3);
        cut.start();
        cut.frame_sending();
        cut.frames_sent(1);
        assert_eq!(cut.handle_flow_control(FlowStatus::Wait, 1, true, 2), result(true));
        assert_eq!(
            (cut.state(), cut.timeout()),
            (TransmitState::Wait, TransmitTimeout::FlowControl)
        );
        assert_eq!(
            cut.frames_sent(1),
            result(true)
                .with_message_param(TransmitMessage::IllegalState, TransmitState::Wait as u8)
        );
        let cut = TransmitProtocolHandler::new(1);
        cut.start();
        cut.frame_sending();
        assert_eq!(
            cut.frame_sending(),
            result(true)
                .with_message_param(TransmitMessage::IllegalState, TransmitState::Wait as u8)
                .with_actions(CANCEL)
        );
        assert!(cut.is_done());
        assert_eq!(cut.state(), TransmitState::Fail);
    }

    #[test]
    fn state_failed_after_tx_timeout() {
        let cut = TransmitProtocolHandler::new(1);
        cut.start();
        cut.frame_sending();
        assert_eq!(
            cut.expired(),
            result(true)
                .with_message(TransmitMessage::TxCallbackTimeoutExpired)
                .with_actions(CANCEL)
        );
        assert!(cut.is_done());
        assert_eq!(cut.error_message(), TransmitMessage::TxCallbackTimeoutExpired);
    }

    #[test]
    fn flow_control_paths() {
        // Wait for flow control after the first frame of a segmented message.
        let cut = TransmitProtocolHandler::new(3);
        cut.start();
        cut.frame_sending();
        assert_eq!(cut.frames_sent(1), result(true));
        assert_eq!(
            (cut.state(), cut.timeout()),
            (TransmitState::Wait, TransmitTimeout::FlowControl)
        );
        assert_eq!(
            cut.handle_flow_control(FlowStatus::Cts, 1, true, 2),
            result(true).with_actions(STORE)
        );
        assert_eq!(
            (cut.state(), cut.timeout()),
            (TransmitState::Send, TransmitTimeout::TxCallback)
        );
        // Wait.
        let cut = TransmitProtocolHandler::new(3);
        cut.start();
        cut.frame_sending();
        cut.frames_sent(1);
        assert_eq!(cut.handle_flow_control(FlowStatus::Wait, 1, true, 2), result(true));
        assert_eq!(
            (cut.state(), cut.timeout()),
            (TransmitState::Wait, TransmitTimeout::FlowControl)
        );
        // Overflow and invalid.
        let cut = TransmitProtocolHandler::new(3);
        cut.start();
        cut.frame_sending();
        cut.frames_sent(1);
        assert_eq!(
            cut.handle_flow_control(FlowStatus::Ovflw, 1, true, 2),
            result(true).with_message(TransmitMessage::FlowControlOverflow)
        );
        assert_eq!(cut.state(), TransmitState::Fail);
        let cut = TransmitProtocolHandler::new(3);
        cut.start();
        cut.frame_sending();
        cut.frames_sent(1);
        assert_eq!(
            cut.handle_flow_control(FlowStatus::Invalid(4), 1, true, 2),
            result(true).with_message(TransmitMessage::FlowControlInvalid)
        );
        // Wait count exceeded.
        let cut = TransmitProtocolHandler::new(3);
        cut.start();
        cut.frame_sending();
        cut.frames_sent(1);
        assert_eq!(cut.handle_flow_control(FlowStatus::Wait, 0, false, 2), result(true));
        assert_eq!(cut.handle_flow_control(FlowStatus::Wait, 0, false, 2), result(true));
        assert_eq!(
            cut.handle_flow_control(FlowStatus::Wait, 0, false, 2),
            result(true).with_message(TransmitMessage::FlowControlWaitCountExceeded)
        );
        assert!(cut.is_done());
        // Ignored where unexpected.
        let cut = TransmitProtocolHandler::new(3);
        cut.start();
        assert_eq!(cut.handle_flow_control(FlowStatus::Wait, 1, true, 2), result(false));
        assert_eq!(
            (cut.state(), cut.timeout()),
            (TransmitState::Send, TransmitTimeout::TxCallback)
        );
    }

    #[test]
    fn flow_control_before_transmit_callback() {
        let cut = TransmitProtocolHandler::new(3);
        cut.start();
        cut.frame_sending();
        assert_eq!(
            cut.handle_flow_control(FlowStatus::Cts, 0, true, 2),
            result(false).with_actions(STORE)
        );
        assert_eq!(cut.frames_sent(1), result(true));
        assert_eq!(cut.frame_sending(), result(true));
        assert_eq!(
            (cut.state(), cut.timeout()),
            (TransmitState::Wait, TransmitTimeout::TxCallback)
        );
        assert_eq!(cut.frames_sent(1), result(true));
        assert_eq!(
            (cut.state(), cut.timeout()),
            (TransmitState::Wait, TransmitTimeout::SeparationTime)
        );
        assert_eq!(cut.handle_flow_control(FlowStatus::Wait, 1, true, 2), result(false));
        assert_eq!(cut.expired(), result(true));
        assert_eq!(cut.state(), TransmitState::Send);
        cut.frame_sending();
        assert_eq!(cut.frames_sent(1), result(true));
        assert_eq!(cut.state(), TransmitState::Success);
        // A second flow control is ignored.
        let cut = TransmitProtocolHandler::new(2);
        cut.start();
        cut.frame_sending();
        cut.frames_sent(1);
        assert_eq!(
            cut.handle_flow_control(FlowStatus::Cts, 0, true, 2),
            result(true).with_actions(STORE)
        );
        cut.frame_sending();
        assert_eq!(cut.handle_flow_control(FlowStatus::Cts, 0, true, 2), result(false));
        // Wait before the callback.
        let cut = TransmitProtocolHandler::new(3);
        cut.start();
        cut.frame_sending();
        assert_eq!(cut.handle_flow_control(FlowStatus::Wait, 1, true, 2), result(false));
        assert_eq!(cut.frames_sent(1), result(true));
        assert_eq!(
            (cut.state(), cut.timeout()),
            (TransmitState::Wait, TransmitTimeout::FlowControl)
        );
    }

    #[test]
    fn consecutive_frames_without_and_with_separation_time_and_block_size() {
        let cut = TransmitProtocolHandler::new(3);
        cut.start();
        cut.frame_sending();
        assert_eq!(
            cut.handle_flow_control(FlowStatus::Cts, 0, false, 2),
            result(false).with_actions(STORE)
        );
        cut.frames_sent(1);
        cut.frame_sending();
        assert_eq!(cut.frames_sent(1), result(true));
        assert_eq!(
            (cut.state(), cut.timeout()),
            (TransmitState::Send, TransmitTimeout::TxCallback)
        );
        cut.frame_sending();
        assert_eq!(cut.frames_sent(1), result(true));
        assert!(cut.is_done());
        assert_eq!(cut.state(), TransmitState::Success);
        // Block size 1: flow control after each block.
        let cut = TransmitProtocolHandler::new(3);
        cut.start();
        cut.frame_sending();
        assert_eq!(
            cut.handle_flow_control(FlowStatus::Cts, 1, false, 2),
            result(false).with_actions(STORE)
        );
        cut.frames_sent(1);
        cut.frame_sending();
        assert_eq!(cut.frames_sent(1), result(true));
        assert_eq!(
            (cut.state(), cut.timeout()),
            (TransmitState::Wait, TransmitTimeout::FlowControl)
        );
        assert_eq!(
            cut.handle_flow_control(FlowStatus::Cts, 1, false, 2),
            result(true).with_actions(STORE)
        );
        cut.frame_sending();
        assert_eq!(cut.frames_sent(1), result(true));
        assert_eq!(cut.state(), TransmitState::Success);
        // Several frames confirmed at once.
        let cut = TransmitProtocolHandler::new(16);
        cut.start();
        cut.frame_sending();
        assert_eq!(
            cut.handle_flow_control(FlowStatus::Cts, 4, false, 2),
            result(false).with_actions(STORE)
        );
        cut.frames_sent(1);
        cut.frame_sending();
        assert_eq!(cut.frames_sent(3), result(true));
        assert_eq!(
            (cut.state(), cut.timeout()),
            (TransmitState::Send, TransmitTimeout::TxCallback)
        );
        cut.frame_sending();
        assert_eq!(cut.frames_sent(1), result(true));
        assert_eq!(
            (cut.state(), cut.timeout()),
            (TransmitState::Wait, TransmitTimeout::FlowControl)
        );
        assert_eq!(
            cut.handle_flow_control(FlowStatus::Cts, 7, false, 2),
            result(true).with_actions(STORE)
        );
        cut.frame_sending();
        assert_eq!(cut.frames_sent(7), result(true));
        assert_eq!(
            (cut.state(), cut.timeout()),
            (TransmitState::Wait, TransmitTimeout::FlowControl)
        );
        assert_eq!(
            cut.handle_flow_control(FlowStatus::Cts, 0, false, 2),
            result(true).with_actions(STORE)
        );
        cut.frame_sending();
        assert_eq!(cut.frames_sent(7), result(true));
        assert!(cut.is_done());
        assert_eq!(cut.state(), TransmitState::Success);
        // Flow control timeout.
        let cut = TransmitProtocolHandler::new(3);
        cut.start();
        cut.frame_sending();
        assert_eq!(
            cut.handle_flow_control(FlowStatus::Cts, 1, false, 2),
            result(false).with_actions(STORE)
        );
        cut.frames_sent(1);
        cut.frame_sending();
        cut.frames_sent(1);
        assert_eq!(
            cut.expired(),
            result(true).with_message(TransmitMessage::FlowControlTimeoutExpired)
        );
        assert!(cut.is_done());
        // Nothing to expire.
        let cut = TransmitProtocolHandler::new(3);
        assert_eq!(cut.expired(), result(false));
        assert_eq!(cut.state(), TransmitState::Initialized);
    }
}
