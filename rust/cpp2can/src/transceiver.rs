// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The transceiver interface and base, ported from `transceiver/ICanTransceiver.h`,
//! `transceiver/ICANTransceiverStateListener.h`, `transceiver/AbstractCANTransceiver.h`
//! and `.cpp`.

use core::cell::Cell;

use openbsw_timer::Lock;

use crate::filter::BitFieldFilter;
use crate::{CanFrame, CanFrameListener, CanFrameSentListener, FilteredCanFrameSentListener};

/// What a transceiver call reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    /// Done.
    Ok,
    /// The controller could not send.
    TxFail,
    /// The controller's queue is full.
    TxHwQueueFull,
    /// The transceiver is closed.
    TxOffline,
    /// The call is not allowed in the current state.
    IllegalState,
    /// No listener can be added.
    NoMoreListenersPossible,
    /// The baud rate is not supported.
    UnsupportedBaudrate,
    /// The controller could not be initialized.
    InitFailed,
}

/// The transceiver's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Not initialized or closed.
    Closed,
    /// Initialized, not open.
    Initialized,
    /// Waking up.
    Waking,
    /// Open.
    Open,
    /// Open, but not sending.
    Muted,
}

/// The bus state a transceiver reports to its state listener.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransceiverState {
    /// Error active.
    Active,
    /// Error passive.
    Passive,
    /// Bus off.
    BusOff,
}

/// The size of the receive queue.
pub const RX_QUEUE_SIZE: usize = 32;
/// The low-speed baud rate.
pub const BAUDRATE_LOWSPEED: u32 = 100_000;
/// The high-speed baud rate.
pub const BAUDRATE_HIGHSPEED: u32 = 500_000;
/// An invalid frame id.
pub const INVALID_FRAME_ID: u16 = 0xFFFF;

/// Told about bus state changes and physical layer errors: the port of
/// `ICANTransceiverStateListener`.
pub trait CanTransceiverStateListener: Sync {
    /// The bus state of `transceiver` changed to `state`.
    fn can_transceiver_state_changed(
        &self,
        transceiver: &dyn CanTransceiver,
        state: TransceiverState,
    );
    /// A physical layer error occurred on `transceiver`.
    fn phy_error_occurred(&self, transceiver: &dyn CanTransceiver);
}

/// A CAN transceiver: the port of `ICanTransceiver`.
///
/// Every method takes `&self`: a transceiver is a `'static` object called from several
/// contexts, so it keeps its state in cells it guards with its platform lock.
pub trait CanTransceiver: Sync {
    /// Initialize the controller.
    fn init(&self) -> ErrorCode;
    /// Shut the controller down.
    fn shutdown(&self);
    /// Open the controller, which then wakes up on `frame`.
    fn open_with(&self, frame: &CanFrame) -> ErrorCode;
    /// Open the controller.
    fn open(&self) -> ErrorCode;
    /// Close the controller.
    fn close(&self) -> ErrorCode;
    /// Stop sending.
    fn mute(&self) -> ErrorCode;
    /// Resume sending.
    fn unmute(&self) -> ErrorCode;
    /// The state.
    fn state(&self) -> State;
    /// The baud rate.
    fn baudrate(&self) -> u32;
    /// How long the controller's queue takes to drain, in milliseconds.
    fn hw_queue_timeout(&self) -> u16;
    /// Send `frame`; a sent frame carries its transmit timestamp afterwards, as the C++
    /// transceiver writes it into the caller's frame.
    fn write(&self, frame: &mut CanFrame) -> ErrorCode;
    /// Send `frame` and tell `listener` once it left.
    fn write_with_listener(
        &self,
        frame: &mut CanFrame,
        listener: &'static dyn CanFrameSentListener,
    ) -> ErrorCode;
    /// Add a receive listener at the end of the list.
    fn add_can_frame_listener(&self, listener: &'static dyn CanFrameListener);
    /// Add a receive listener at the front of the list.
    fn add_vip_can_frame_listener(&self, listener: &'static dyn CanFrameListener);
    /// Remove a receive listener.
    fn remove_can_frame_listener(&self, listener: &dyn CanFrameListener);
    /// The bus id.
    fn bus_id(&self) -> u8;
    /// Add a listener told about every sent frame.
    fn add_can_frame_sent_listener(&self, listener: &'static dyn FilteredCanFrameSentListener);
    /// Remove a sent listener.
    fn remove_can_frame_sent_listener(&self, listener: &dyn FilteredCanFrameSentListener);
    /// The bus state.
    fn can_transceiver_state(&self) -> TransceiverState;
    /// Set the one state listener.
    fn set_state_listener(&self, listener: &'static dyn CanTransceiverStateListener);
    /// Remove the state listener.
    fn remove_state_listener(&self);
}

/// The bookkeeping every transceiver shares: the port of `AbstractCANTransceiver`.
///
/// A concrete transceiver embeds one and delegates the listener and state methods of
/// [`CanTransceiver`] to it. The lists are changed under `L`, the platform's lock.
pub struct AbstractCanTransceiver<L: Lock> {
    filter: BitFieldFilter,
    listeners: Cell<Option<&'static dyn CanFrameListener>>,
    sent_listeners: Cell<Option<&'static dyn FilteredCanFrameSentListener>>,
    baudrate: Cell<u32>,
    state: Cell<State>,
    bus_id: u8,
    state_listener: Cell<Option<&'static dyn CanTransceiverStateListener>>,
    transceiver_state: Cell<TransceiverState>,
    system_time_us: &'static (dyn Fn() -> u32 + Sync),
    _lock: core::marker::PhantomData<fn() -> L>,
}

// SAFETY: the cells are changed under `L` or on the owning transceiver's context, the
// same discipline the C++ base relies on.
unsafe impl<L: Lock> Sync for AbstractCanTransceiver<L> {}

impl<L: Lock> AbstractCanTransceiver<L> {
    /// A closed transceiver on `bus_id`, stamping sent frames with `system_time_us`
    /// (`getSystemTimeUs32Bit`).
    pub const fn new(bus_id: u8, system_time_us: &'static (dyn Fn() -> u32 + Sync)) -> Self {
        Self {
            filter: BitFieldFilter::new(),
            listeners: Cell::new(None),
            sent_listeners: Cell::new(None),
            baudrate: Cell::new(0),
            state: Cell::new(State::Closed),
            bus_id,
            state_listener: Cell::new(None),
            transceiver_state: Cell::new(TransceiverState::Active),
            system_time_us,
            _lock: core::marker::PhantomData,
        }
    }

    /// The state.
    pub fn state(&self) -> State {
        self.state.get()
    }

    /// Set the state.
    pub fn set_state(&self, state: State) {
        self.state.set(state);
    }

    /// Whether the state is `state`.
    pub fn is_in_state(&self, state: State) -> bool {
        self.state.get() == state
    }

    /// The baud rate.
    pub fn baudrate(&self) -> u32 {
        self.baudrate.get()
    }

    /// Set the baud rate.
    pub fn set_baudrate(&self, baudrate: u32) {
        self.baudrate.set(baudrate);
    }

    /// The bus id.
    pub fn bus_id(&self) -> u8 {
        self.bus_id
    }

    /// The bus state.
    pub fn can_transceiver_state(&self) -> TransceiverState {
        self.transceiver_state.get()
    }

    /// Set the bus state (without notifying).
    pub fn set_transceiver_state(&self, state: TransceiverState) {
        self.transceiver_state.set(state);
    }

    /// The acceptance mask: every listener's filter merged.
    pub fn filter(&self) -> &BitFieldFilter {
        &self.filter
    }

    /// Set the one state listener.
    pub fn set_state_listener(&self, listener: &'static dyn CanTransceiverStateListener) {
        self.state_listener.set(Some(listener));
    }

    /// Remove the state listener.
    pub fn remove_state_listener(&self) {
        self.state_listener.set(None);
    }

    /// Add a receive listener at the end of the list, unless it is listed already.
    pub fn add_can_frame_listener(&self, listener: &'static dyn CanFrameListener) {
        let _lock = L::lock();
        if self.contains_listener(listener) {
            return;
        }
        listener.node().next.set(None);
        match self.last_listener() {
            None => self.listeners.set(Some(listener)),
            Some(last) => last.node().next.set(Some(listener)),
        }
        listener.filter().accept_merger(&self.filter);
    }

    /// Add a receive listener at the front of the list, unless it is listed already.
    pub fn add_vip_can_frame_listener(&self, listener: &'static dyn CanFrameListener) {
        let _lock = L::lock();
        if self.contains_listener(listener) {
            return;
        }
        listener.node().next.set(self.listeners.get());
        self.listeners.set(Some(listener));
        listener.filter().accept_merger(&self.filter);
    }

    /// Remove a receive listener.
    pub fn remove_can_frame_listener(&self, listener: &dyn CanFrameListener) {
        let _lock = L::lock();
        let mut prev: Option<&'static dyn CanFrameListener> = None;
        let mut current = self.listeners.get();
        while let Some(candidate) = current {
            if core::ptr::addr_eq(candidate, listener) {
                let next = candidate.node().next.take();
                match prev {
                    None => self.listeners.set(next),
                    Some(prev) => prev.node().next.set(next),
                }
                return;
            }
            prev = Some(candidate);
            current = candidate.node().next.get();
        }
    }

    /// Add a sent listener at the front, as the C++ forward list does, unless it is listed
    /// already: relinking a listed node would close the list into a cycle (the ETL list
    /// asserts on a linked node).
    pub fn add_can_frame_sent_listener(&self, listener: &'static dyn FilteredCanFrameSentListener) {
        let _lock = L::lock();
        let mut current = self.sent_listeners.get();
        while let Some(candidate) = current {
            if core::ptr::addr_eq(candidate, listener) {
                return;
            }
            current = candidate.node().next.get();
        }
        listener.node().next.set(self.sent_listeners.get());
        self.sent_listeners.set(Some(listener));
    }

    /// Remove a sent listener.
    pub fn remove_can_frame_sent_listener(&self, listener: &dyn FilteredCanFrameSentListener) {
        let _lock = L::lock();
        let mut prev: Option<&'static dyn FilteredCanFrameSentListener> = None;
        let mut current = self.sent_listeners.get();
        while let Some(candidate) = current {
            if core::ptr::addr_eq(candidate, listener) {
                let next = candidate.node().next.take();
                match prev {
                    None => self.sent_listeners.set(next),
                    Some(prev) => prev.node().next.set(next),
                }
                return;
            }
            prev = Some(candidate);
            current = candidate.node().next.get();
        }
    }

    /// Hand a received frame to every listener whose filter accepts it; nothing is
    /// received while closed.
    pub fn notify_listeners(&self, frame: &CanFrame) {
        if self.state.get() == State::Closed {
            return;
        }
        let mut current = self.listeners.get();
        while let Some(listener) = current {
            if listener.filter().matches(frame.id()) {
                listener.frame_received(frame);
            }
            current = listener.node().next.get();
        }
    }

    /// Tell every sent listener about `frame`, first stamping it with the current time: the
    /// C++ writes the timestamp into the caller's frame, so does this.
    pub fn notify_sent_listeners(&self, frame: &mut CanFrame) {
        if self.sent_listeners.get().is_none() {
            return;
        }
        frame.set_timestamp((self.system_time_us)());
        let mut current = self.sent_listeners.get();
        while let Some(listener) = current {
            listener.can_frame_sent(frame);
            current = listener.node().next.get();
        }
    }

    /// Tell the state listener about a physical layer error on `transceiver`.
    pub fn notify_state_listener_with_phy_error(&self, transceiver: &dyn CanTransceiver) {
        if let Some(listener) = self.state_listener.get() {
            listener.phy_error_occurred(transceiver);
        }
    }

    /// Tell the state listener that `transceiver` is in `state`.
    pub fn notify_state_listener_with_state(
        &self,
        transceiver: &dyn CanTransceiver,
        state: TransceiverState,
    ) {
        if let Some(listener) = self.state_listener.get() {
            listener.can_transceiver_state_changed(transceiver, state);
        }
    }

    fn contains_listener(&self, listener: &dyn CanFrameListener) -> bool {
        let mut current = self.listeners.get();
        while let Some(candidate) = current {
            if core::ptr::addr_eq(candidate, listener) {
                return true;
            }
            current = candidate.node().next.get();
        }
        false
    }

    fn last_listener(&self) -> Option<&'static dyn CanFrameListener> {
        let mut last = self.listeners.get()?;
        while let Some(next) = last.node().next.get() {
            last = next;
        }
        Some(last)
    }
}

// Ported from cpp2can/test/src/can/transceiver/AbstractCANTransceiverTest.cpp.
#[cfg(test)]
mod tests {
    extern crate std;

    use std::boxed::Box;
    use std::cell::RefCell;
    use std::vec::Vec;

    use openbsw_timer::NoLock;

    use super::*;
    use crate::filter::{
        Filter, IntervalFilter, StaticBitFieldFilter, StaticMask, static_bit_fields_equal,
    };
    use crate::{ListenerNode, SentListenerNode};

    static NOW: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    fn now() -> u32 {
        NOW.load(core::sync::atomic::Ordering::Relaxed)
    }
    fn set_now(value: u32) {
        NOW.store(value, core::sync::atomic::Ordering::Relaxed);
    }

    /// The port of `AbstractCANTransceiverMock`: the base plus `inject`.
    struct MockTransceiver {
        base: AbstractCanTransceiver<NoLock>,
        calls: RefCell<Vec<&'static str>>,
    }
    // SAFETY: single-threaded test object.
    unsafe impl Sync for MockTransceiver {}

    impl MockTransceiver {
        fn new(bus_id: u8) -> &'static Self {
            Box::leak(Box::new(Self {
                base: AbstractCanTransceiver::new(bus_id, &now),
                calls: RefCell::new(Vec::new()),
            }))
        }
        fn inject(&self, frame: &CanFrame) {
            self.base.notify_listeners(frame);
        }
        fn set_transceiver_state2(&self, state: TransceiverState) {
            self.base.set_transceiver_state(state);
            self.base.notify_state_listener_with_state(self, state);
        }
        fn take_calls(&self) -> Vec<&'static str> {
            core::mem::take(&mut *self.calls.borrow_mut())
        }
    }

    impl CanTransceiver for MockTransceiver {
        fn init(&self) -> ErrorCode {
            self.calls.borrow_mut().push("init");
            ErrorCode::Ok
        }
        fn shutdown(&self) {}
        fn open_with(&self, _frame: &CanFrame) -> ErrorCode {
            ErrorCode::IllegalState
        }
        fn open(&self) -> ErrorCode {
            self.calls.borrow_mut().push("open");
            self.base.set_state(State::Open);
            self.base.notify_state_listener_with_state(self, TransceiverState::Active);
            self.base.notify_state_listener_with_phy_error(self);
            ErrorCode::Ok
        }
        fn close(&self) -> ErrorCode {
            self.base.set_state(State::Closed);
            ErrorCode::Ok
        }
        fn mute(&self) -> ErrorCode {
            ErrorCode::Ok
        }
        fn unmute(&self) -> ErrorCode {
            ErrorCode::Ok
        }
        fn state(&self) -> State {
            self.base.state()
        }
        fn baudrate(&self) -> u32 {
            self.base.baudrate()
        }
        fn hw_queue_timeout(&self) -> u16 {
            0
        }
        fn write(&self, frame: &mut CanFrame) -> ErrorCode {
            self.calls.borrow_mut().push("write");
            self.base.notify_sent_listeners(frame);
            ErrorCode::Ok
        }
        fn write_with_listener(
            &self,
            frame: &mut CanFrame,
            listener: &'static dyn CanFrameSentListener,
        ) -> ErrorCode {
            self.calls.borrow_mut().push("write_with_listener");
            listener.can_frame_sent(frame);
            ErrorCode::Ok
        }
        fn add_can_frame_listener(&self, listener: &'static dyn CanFrameListener) {
            self.base.add_can_frame_listener(listener);
        }
        fn add_vip_can_frame_listener(&self, listener: &'static dyn CanFrameListener) {
            self.base.add_vip_can_frame_listener(listener);
        }
        fn remove_can_frame_listener(&self, listener: &dyn CanFrameListener) {
            self.base.remove_can_frame_listener(listener);
        }
        fn bus_id(&self) -> u8 {
            self.base.bus_id()
        }
        fn add_can_frame_sent_listener(&self, listener: &'static dyn FilteredCanFrameSentListener) {
            self.base.add_can_frame_sent_listener(listener);
        }
        fn remove_can_frame_sent_listener(&self, listener: &dyn FilteredCanFrameSentListener) {
            self.base.remove_can_frame_sent_listener(listener);
        }
        fn can_transceiver_state(&self) -> TransceiverState {
            self.base.can_transceiver_state()
        }
        fn set_state_listener(&self, listener: &'static dyn CanTransceiverStateListener) {
            self.base.set_state_listener(listener);
        }
        fn remove_state_listener(&self) {
            self.base.remove_state_listener();
        }
    }

    /// A listener with any filter that records what it received.
    struct Listener<F: Filter + Sync> {
        filter: F,
        received: RefCell<Vec<CanFrame>>,
        node: ListenerNode,
    }
    // SAFETY: single-threaded test object.
    unsafe impl<F: Filter + Sync> Sync for Listener<F> {}

    impl<F: Filter + Sync> Listener<F> {
        fn new(filter: F) -> &'static Self {
            Box::leak(Box::new(Self {
                filter,
                received: RefCell::new(Vec::new()),
                node: ListenerNode::new(),
            }))
        }
        fn take(&self) -> Vec<CanFrame> {
            core::mem::take(&mut *self.received.borrow_mut())
        }
    }

    impl<F: Filter + Sync> CanFrameListener for Listener<F> {
        fn frame_received(&self, frame: &CanFrame) {
            self.received.borrow_mut().push(*frame);
        }
        fn filter(&self) -> &dyn Filter {
            &self.filter
        }
        fn node(&self) -> &ListenerNode {
            &self.node
        }
    }

    /// The port of `aBitFieldFilter`: every mask byte is `mask`.
    struct ByteMask(Cell<u8>);
    // SAFETY: single-threaded test object.
    unsafe impl Sync for ByteMask {}
    impl StaticMask for ByteMask {
        fn mask_value(&self, _byte_index: u16) -> u8 {
            self.0.get()
        }
    }

    fn bit_field_listener() -> &'static Listener<BitFieldFilter> {
        Listener::new(BitFieldFilter::new())
    }
    fn interval_listener() -> &'static Listener<IntervalFilter> {
        Listener::new(IntervalFilter::new())
    }
    fn static_listener() -> &'static Listener<StaticBitFieldFilter<ByteMask>> {
        Listener::new(StaticBitFieldFilter::new(ByteMask(Cell::new(0xFF))))
    }

    /// The port of `tStateChangeListener`: records states, errors and sent frames.
    struct StateListener {
        states: RefCell<Vec<TransceiverState>>,
        phy_errors: Cell<u32>,
        sent: RefCell<Vec<CanFrame>>,
    }
    // SAFETY: single-threaded test object.
    unsafe impl Sync for StateListener {}
    impl StateListener {
        fn new() -> &'static Self {
            Box::leak(Box::new(Self {
                states: RefCell::new(Vec::new()),
                phy_errors: Cell::new(0),
                sent: RefCell::new(Vec::new()),
            }))
        }
    }
    impl CanTransceiverStateListener for StateListener {
        fn can_transceiver_state_changed(
            &self,
            _transceiver: &dyn CanTransceiver,
            state: TransceiverState,
        ) {
            self.states.borrow_mut().push(state);
        }
        fn phy_error_occurred(&self, _transceiver: &dyn CanTransceiver) {
            self.phy_errors.set(self.phy_errors.get() + 1);
        }
    }
    impl CanFrameSentListener for StateListener {
        fn can_frame_sent(&self, frame: &CanFrame) {
            self.sent.borrow_mut().push(*frame);
        }
    }

    /// The port of `tSendFrameListener` and `FilteredCANFrameSentListenerMock`.
    struct SentListener {
        filter: StaticBitFieldFilter<ByteMask>,
        sent: RefCell<Vec<CanFrame>>,
        node: SentListenerNode,
    }
    // SAFETY: single-threaded test object.
    unsafe impl Sync for SentListener {}
    impl SentListener {
        fn new() -> &'static Self {
            Box::leak(Box::new(Self {
                filter: StaticBitFieldFilter::new(ByteMask(Cell::new(0xFF))),
                sent: RefCell::new(Vec::new()),
                node: SentListenerNode::new(),
            }))
        }
        fn take(&self) -> Vec<CanFrame> {
            core::mem::take(&mut *self.sent.borrow_mut())
        }
    }
    impl FilteredCanFrameSentListener for SentListener {
        fn can_frame_sent(&self, frame: &CanFrame) {
            self.sent.borrow_mut().push(*frame);
        }
        fn filter(&self) -> &dyn Filter {
            &self.filter
        }
        fn node(&self) -> &SentListenerNode {
            &self.node
        }
    }

    #[test]
    fn constructor() {
        let transceiver = MockTransceiver::new(123);
        assert_eq!(transceiver.bus_id(), 123);
        assert_eq!(transceiver.state(), State::Closed);
        assert_eq!(transceiver.can_transceiver_state(), TransceiverState::Active);
    }

    #[test]
    fn add_can_frame_listener_merges_the_filter_once() {
        let transceiver = MockTransceiver::new(0);
        let listener = bit_field_listener();
        listener.filter.add(0x123);
        transceiver.add_can_frame_listener(listener);
        transceiver.add_can_frame_listener(listener);
        assert!(transceiver.base.filter().matches(0x123));
        assert!(!transceiver.base.filter().matches(0x124));
        let listener2 = interval_listener();
        listener2.filter.add_range(0x200, 0x20F);
        transceiver.add_vip_can_frame_listener(listener2);
        transceiver.add_vip_can_frame_listener(listener2);
        assert!(transceiver.base.filter().matches(0x205));
        // Adding twice linked each once: removing once unlinks it.
        transceiver.remove_can_frame_listener(listener);
        transceiver.remove_can_frame_listener(listener2);
        transceiver.base.set_state(State::Open);
        transceiver.inject(&CanFrame::with_id(0x123));
        transceiver.inject(&CanFrame::with_id(0x205));
        assert!(listener.take().is_empty());
        assert!(listener2.take().is_empty());
    }

    #[test]
    fn remove_can_frame_listener() {
        let transceiver = MockTransceiver::new(0);
        let listener = bit_field_listener();
        transceiver.add_can_frame_listener(listener);
        transceiver.add_can_frame_listener(listener);
        transceiver.remove_can_frame_listener(listener);
        transceiver.add_can_frame_listener(listener);
        transceiver.remove_can_frame_listener(listener);
        let listener2 = interval_listener();
        transceiver.add_can_frame_listener(listener2);
        transceiver.add_can_frame_listener(listener2);
        transceiver.remove_can_frame_listener(listener2);
        transceiver.add_can_frame_listener(listener2);
        transceiver.remove_can_frame_listener(listener2);
        transceiver.add_vip_can_frame_listener(listener2);
        transceiver.remove_can_frame_listener(listener2);
        // Removing an unknown listener is harmless.
        transceiver.remove_can_frame_listener(listener2);
        assert!(transceiver.base.listeners.get().is_none());
    }

    #[test]
    fn state_listeners() {
        let transceiver = MockTransceiver::new(0);
        let state_listener = StateListener::new();
        assert_eq!(transceiver.init(), ErrorCode::Ok);
        assert_eq!(transceiver.state(), State::Closed);
        transceiver.set_state_listener(state_listener);
        assert_eq!(transceiver.open(), ErrorCode::Ok);
        assert_eq!(transceiver.state(), State::Open);
        assert_eq!(transceiver.can_transceiver_state(), TransceiverState::Active);
        assert_eq!(state_listener.states.borrow().as_slice(), [TransceiverState::Active]);
        assert_eq!(state_listener.phy_errors.get(), 1);
        transceiver.set_transceiver_state2(TransceiverState::BusOff);
        assert_eq!(
            state_listener.states.borrow().as_slice(),
            [TransceiverState::Active, TransceiverState::BusOff]
        );
        transceiver.remove_state_listener();
        transceiver.set_transceiver_state2(TransceiverState::Active);
        assert_eq!(state_listener.states.borrow().len(), 2);
        assert_eq!(transceiver.take_calls(), ["init", "open"]);
    }

    #[test]
    fn send_listeners() {
        let transceiver = MockTransceiver::new(0);
        let send_listener = StateListener::new();
        let mut frame = CanFrame::with_id(0x555);
        assert_eq!(transceiver.write_with_listener(&mut frame, send_listener), ErrorCode::Ok);
        assert_eq!(send_listener.sent.borrow().as_slice(), [frame]);
        let send_listener2 = SentListener::new();
        transceiver.add_can_frame_sent_listener(send_listener2);
        set_now(77);
        assert_eq!(transceiver.write(&mut frame), ErrorCode::Ok);
        assert_eq!(send_listener2.take(), [frame]);
        // The transmit timestamp is written into the caller's frame, as in C++.
        assert_eq!(frame.timestamp(), 77);
        transceiver.remove_can_frame_sent_listener(send_listener2);
        assert_eq!(transceiver.take_calls(), ["write_with_listener", "write"]);
    }

    #[test]
    fn notify_listeners() {
        let transceiver = MockTransceiver::new(0);
        assert_eq!(transceiver.init(), ErrorCode::Ok);
        assert_eq!(transceiver.state(), State::Closed);
        let frame = CanFrame::from_payload(0x555, &[0x00, 0x01, 0x02, 0x03, 0x04, 0x05]);
        let frame1 = CanFrame::from_payload(0x123, &[0x06, 0x05, 0x04, 0x03, 0x02, 0x01]);
        let listener1 = bit_field_listener();
        let listener2 = bit_field_listener();
        let listener3 = interval_listener();
        let listener4 = interval_listener();
        let listener5 = static_listener();
        listener1.filter.add(0x555);
        listener3.filter.add_range(0x000, 0x7FF);
        listener4.filter.add_range(0x000, 0x7FF);
        listener5.filter.add(0x555);
        listener5.filter.add_range(0, 0x700);
        transceiver.add_can_frame_listener(listener3);
        // Closed: nothing is received.
        transceiver.inject(&frame);
        assert!(listener3.take().is_empty());
        assert_eq!(transceiver.open(), ErrorCode::Ok);
        assert_eq!(transceiver.state(), State::Open);
        transceiver.add_can_frame_listener(listener1);
        transceiver.add_can_frame_listener(listener2);
        transceiver.add_can_frame_listener(listener4);
        transceiver.add_can_frame_listener(listener5);
        transceiver.inject(&frame);
        assert_eq!(listener1.take(), [frame]);
        assert!(listener2.take().is_empty());
        assert_eq!(listener3.take(), [frame]);
        assert_eq!(listener4.take(), [frame]);
        assert_eq!(listener5.take(), [frame]);
        transceiver.inject(&frame1);
        assert!(listener1.take().is_empty());
        assert_eq!(listener3.take(), [frame1]);
        assert_eq!(listener4.take(), [frame1]);
        assert_eq!(listener5.take(), [frame1]);
        // An extended id matches no base filter.
        let mut extended = frame;
        extended.set_id(0x17000080);
        transceiver.inject(&extended);
        assert!(listener1.take().is_empty());
        assert!(listener3.take().is_empty());
        assert!(listener5.take().is_empty());
        // The static filter is what its mask says, whatever was added or cleared.
        listener5.filter.clear();
        listener5.filter.add(0x555);
        assert!(static_bit_fields_equal(listener5.filter.mask(), &ByteMask(Cell::new(0xFF))));
        assert!(!static_bit_fields_equal(listener5.filter.mask(), &ByteMask(Cell::new(0))));
        for listener in
            [listener1 as &dyn CanFrameListener, listener2, listener3, listener4, listener5]
        {
            transceiver.remove_can_frame_listener(listener);
        }
        assert!(transceiver.base.listeners.get().is_none());
    }

    #[test]
    fn vip_listeners_come_first() {
        let transceiver = MockTransceiver::new(0);
        transceiver.base.set_state(State::Open);
        let order: &'static RefCell<Vec<u8>> = Box::leak(Box::new(RefCell::new(Vec::new())));
        struct Ordered(u8, &'static RefCell<Vec<u8>>, IntervalFilter, ListenerNode);
        // SAFETY: single-threaded test object.
        unsafe impl Sync for Ordered {}
        impl CanFrameListener for Ordered {
            fn frame_received(&self, _frame: &CanFrame) {
                self.1.borrow_mut().push(self.0);
            }
            fn filter(&self) -> &dyn Filter {
                &self.2
            }
            fn node(&self) -> &ListenerNode {
                &self.3
            }
        }
        let make = |n| -> &'static Ordered {
            Box::leak(Box::new(Ordered(
                n,
                order,
                IntervalFilter::with_range(0, 0x7FF),
                ListenerNode::new(),
            )))
        };
        transceiver.add_can_frame_listener(make(1));
        transceiver.add_can_frame_listener(make(2));
        transceiver.add_vip_can_frame_listener(make(3));
        transceiver.add_can_frame_listener(make(4));
        transceiver.inject(&CanFrame::with_id(0x100));
        assert_eq!(order.borrow().as_slice(), [3, 1, 2, 4]);
    }

    #[test]
    fn notify_sent_listeners() {
        let mut frame = CanFrame::from_payload(0x555, &[0x00, 0x01, 0x02, 0x03, 0x04, 0x05]);
        let transceiver = MockTransceiver::new(0);
        // Without listeners nothing is stamped.
        transceiver.base.notify_sent_listeners(&mut frame);
        assert_eq!(frame.timestamp(), 0);
        let listener1 = SentListener::new();
        let listener2 = SentListener::new();
        transceiver.add_can_frame_sent_listener(listener1);
        transceiver.add_can_frame_sent_listener(listener2);
        set_now(150);
        transceiver.base.notify_sent_listeners(&mut frame);
        let sent1 = listener1.take();
        let sent2 = listener2.take();
        assert_eq!(sent1, [frame]);
        assert_eq!(sent2, [frame]);
        assert_eq!(sent1[0].timestamp(), 150);
        assert_eq!(sent2[0].timestamp(), 150);
        // The caller's frame carries the timestamp too.
        assert_eq!(frame.timestamp(), 150);
        transceiver.remove_can_frame_sent_listener(listener1);
        set_now(180);
        transceiver.base.notify_sent_listeners(&mut frame);
        assert!(listener1.take().is_empty());
        let sent2 = listener2.take();
        assert_eq!(sent2, [frame]);
        assert_eq!(sent2[0].timestamp(), 180);
        transceiver.remove_can_frame_sent_listener(listener2);
    }

    // Adding a listed sent listener again, even one with a successor, changes nothing; the
    // list does not close into a cycle.
    #[test]
    fn adding_a_sent_listener_twice_keeps_the_list() {
        let mut frame = CanFrame::with_id(0x555);
        let transceiver = MockTransceiver::new(0);
        let listener1 = SentListener::new();
        let listener2 = SentListener::new();
        transceiver.add_can_frame_sent_listener(listener1);
        transceiver.add_can_frame_sent_listener(listener2);
        // listener2 is first and has listener1 as its successor.
        transceiver.add_can_frame_sent_listener(listener2);
        transceiver.add_can_frame_sent_listener(listener1);
        transceiver.base.notify_sent_listeners(&mut frame);
        assert_eq!(listener1.take().len(), 1);
        assert_eq!(listener2.take().len(), 1);
        transceiver.remove_can_frame_sent_listener(listener1);
        transceiver.remove_can_frame_sent_listener(listener2);
    }

    #[test]
    fn set_frame_sent_listener() {
        let mut frame = CanFrame::from_payload(0x555, &[0x00, 0x01, 0x02, 0x03, 0x04, 0x05]);
        let transceiver = MockTransceiver::new(0);
        let listener1 = SentListener::new();
        transceiver.add_can_frame_sent_listener(listener1);
        set_now(150);
        transceiver.base.notify_sent_listeners(&mut frame);
        let sent = listener1.take();
        assert_eq!(sent, [frame]);
        assert_eq!(sent[0].timestamp(), 150);
        transceiver.remove_can_frame_sent_listener(listener1);
        transceiver.base.notify_sent_listeners(&mut frame);
        assert!(listener1.take().is_empty());
    }
}
