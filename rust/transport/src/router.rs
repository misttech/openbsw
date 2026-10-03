// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The simple router, ported from `routing/TransportRouterSimple.h` and `.cpp` as the
//! Zephyr demo builds it: a request from the CAN bus goes to the diagnostic layer, and the
//! answer from there goes back to the bus it came from.

use core::cell::Cell;
use core::marker::PhantomData;

use openbsw_timer::Lock;
use openbsw_util::format::Arg;
use openbsw_util::{log_debug, log_error};

use crate::layer::{LayerErrorCode, TransportLayer};
use crate::message::{
    ProviderErrorCode, ReceiveResult, TransportMessage, TransportMessageListener,
    TransportMessageProcessedListener, TransportMessageProvider,
};
use crate::{TPROUTER, bus_name};

/// The number of full-size buffers.
pub const NUM_BUFFERS: usize = 3;
/// The number of buffers for functional requests.
pub const NUM_FUNCTIONAL_BUFFERS: usize = 8;
/// The size of a full-size buffer.
pub const BUFFER_SIZE: usize = 0xFFF;
/// The size of a functional buffer.
pub const FUNCTIONAL_BUFFER_SIZE: usize = 8;

/// The application's bus ids and addresses the router routes by (`busid` and
/// `TransportConfiguration`).
#[derive(Clone, Copy, Debug)]
pub struct RouterConfig {
    /// The diagnostic application's bus (`busid::SELFDIAG`).
    pub selfdiag_bus_id: u8,
    /// The CAN bus (`busid::CAN_0`).
    pub can_bus_id: u8,
    /// The functional target address, compared with the low byte of a target id
    /// (`FUNCTIONAL_ALL_ISO14229`).
    pub functional_address: u8,
    /// The largest functional request (`MAX_FUNCTIONAL_MESSAGE_PAYLOAD_SIZE`).
    pub max_functional_payload: u16,
}

/// Three large and eight small message buffers, and the layers they travel between.
pub struct TransportRouterSimple<L: Lock + 'static> {
    this: &'static Self,
    config: RouterConfig,
    locked: [Cell<bool>; NUM_BUFFERS],
    functional_locked: [Cell<bool>; NUM_FUNCTIONAL_BUFFERS],
    buffers: [[Cell<u8>; BUFFER_SIZE]; NUM_BUFFERS],
    functional_buffers: [[Cell<u8>; FUNCTIONAL_BUFFER_SIZE]; NUM_FUNCTIONAL_BUFFERS],
    messages: [TransportMessage; NUM_BUFFERS],
    functional_messages: [TransportMessage; NUM_FUNCTIONAL_BUFFERS],
    layers: Cell<Option<&'static dyn TransportLayer>>,
    bus_id_to_reply: Cell<u8>,
    _lock: PhantomData<fn() -> L>,
}

// SAFETY: the buffer locks change under `L`, the platform lock; the layer list changes
// during lifecycle transitions; the reply bus on the receiving context.
unsafe impl<L: Lock + 'static> Sync for TransportRouterSimple<L> {}

impl<L: Lock + 'static> TransportRouterSimple<L> {
    /// A router, given its own `'static` address (its messages are over its buffers).
    pub const fn new(this: &'static Self, config: RouterConfig) -> Self {
        Self {
            this,
            config,
            locked: [const { Cell::new(false) }; NUM_BUFFERS],
            functional_locked: [const { Cell::new(false) }; NUM_FUNCTIONAL_BUFFERS],
            buffers: [const { [const { Cell::new(0) }; BUFFER_SIZE] }; NUM_BUFFERS],
            functional_buffers: [const { [const { Cell::new(0) }; FUNCTIONAL_BUFFER_SIZE] };
                NUM_FUNCTIONAL_BUFFERS],
            messages: [
                TransportMessage::with_buffer(&this.buffers[0]),
                TransportMessage::with_buffer(&this.buffers[1]),
                TransportMessage::with_buffer(&this.buffers[2]),
            ],
            functional_messages: [
                TransportMessage::with_buffer(&this.functional_buffers[0]),
                TransportMessage::with_buffer(&this.functional_buffers[1]),
                TransportMessage::with_buffer(&this.functional_buffers[2]),
                TransportMessage::with_buffer(&this.functional_buffers[3]),
                TransportMessage::with_buffer(&this.functional_buffers[4]),
                TransportMessage::with_buffer(&this.functional_buffers[5]),
                TransportMessage::with_buffer(&this.functional_buffers[6]),
                TransportMessage::with_buffer(&this.functional_buffers[7]),
            ],
            layers: Cell::new(None),
            bus_id_to_reply: Cell::new(config.selfdiag_bus_id),
            _lock: PhantomData,
        }
    }

    /// Forget every layer.
    pub fn init(&self) {
        self.layers.set(None);
    }

    /// Forget every layer.
    pub fn shutdown(&self) {
        self.layers.set(None);
    }

    /// Route the messages of `layer`; a second layer for the same bus is refused.
    pub fn add_transport_layer(&'static self, layer: &'static dyn TransportLayer) {
        let mut current = self.layers.get();
        let mut last: Option<&'static dyn TransportLayer> = None;
        while let Some(candidate) = current {
            if candidate.bus_id() == layer.bus_id() {
                log_error!(
                    TPROUTER,
                    b"TpLayer for bus %s must not be registered multiple times",
                    Arg::Str(Some(bus_name(candidate.bus_id())))
                );
                return;
            }
            last = Some(candidate);
            current = candidate.node().next.get();
        }
        layer.providing_listener_helper().set_listener(Some(self));
        layer.providing_listener_helper().set_provider(Some(self));
        layer.node().next.set(None);
        match last {
            None => self.layers.set(Some(layer)),
            Some(last) => last.node().next.set(Some(layer)),
        }
    }

    /// Stop routing the messages of `layer`.
    pub fn remove_transport_layer(&self, layer: &'static dyn TransportLayer) {
        let mut prev: Option<&'static dyn TransportLayer> = None;
        let mut current = self.layers.get();
        while let Some(candidate) = current {
            if core::ptr::addr_eq(candidate, layer) {
                layer.providing_listener_helper().set_listener(None);
                layer.providing_listener_helper().set_provider(None);
                let next = candidate.node().next.take();
                match prev {
                    None => self.layers.set(next),
                    Some(prev) => prev.node().next.set(next),
                }
                return;
            }
            prev = Some(candidate);
            current = candidate.node().next.get();
        }
    }

    fn forward_message_to_transport_layer(
        &self,
        message: &'static TransportMessage,
        dest_bus_id: u8,
        notification: Option<&'static dyn TransportMessageProcessedListener>,
    ) -> LayerErrorCode {
        let mut current = self.layers.get();
        while let Some(layer) = current {
            if layer.bus_id() == dest_bus_id {
                return layer.send(message, notification);
            }
            current = layer.node().next.get();
        }
        LayerErrorCode::Ok
    }
}

impl<L: Lock + 'static> TransportMessageProvider for TransportRouterSimple<L> {
    fn get_transport_message(
        &self,
        _src_bus_id: u8,
        source_address: u16,
        target_address: u16,
        size: u16,
        _peek: &[u8],
    ) -> Result<&'static TransportMessage, ProviderErrorCode> {
        log_debug!(
            TPROUTER,
            b"TransportRouterSimple::getTransportMessage : sourceId 0x%x, targetId 0x%x",
            source_address,
            target_address
        );
        let _lock = L::lock();
        let this = self.this;
        if (target_address as u8) == self.config.functional_address
            && size <= self.config.max_functional_payload
        {
            for (index, locked) in self.functional_locked.iter().enumerate() {
                if !locked.get() {
                    this.functional_messages[index].init(&this.functional_buffers[index]);
                    locked.set(true);
                    return Ok(&this.functional_messages[index]);
                }
            }
        }
        if usize::from(size) > BUFFER_SIZE {
            return Err(ProviderErrorCode::SizeTooLarge);
        }
        for (index, locked) in self.locked.iter().enumerate() {
            if !locked.get() {
                this.messages[index].init(&this.buffers[index]);
                locked.set(true);
                return Ok(&this.messages[index]);
            }
        }
        Err(ProviderErrorCode::NoMsgAvailable)
    }

    fn release_transport_message(&self, message: &'static TransportMessage) {
        let _lock = L::lock();
        for (index, candidate) in self.messages.iter().enumerate() {
            if core::ptr::eq(candidate, message) {
                self.locked[index].set(false);
                return;
            }
        }
        for (index, candidate) in self.functional_messages.iter().enumerate() {
            if core::ptr::eq(candidate, message) {
                self.functional_locked[index].set(false);
                return;
            }
        }
    }
}

impl<L: Lock + 'static> TransportMessageListener for TransportRouterSimple<L> {
    fn message_received(
        &self,
        source_bus_id: u8,
        message: &'static TransportMessage,
        notification: Option<&'static dyn TransportMessageProcessedListener>,
    ) -> ReceiveResult {
        let result = if source_bus_id == self.config.can_bus_id {
            self.bus_id_to_reply.set(source_bus_id);
            self.forward_message_to_transport_layer(
                message,
                self.config.selfdiag_bus_id,
                notification,
            )
        } else if source_bus_id == self.config.selfdiag_bus_id
            && self.bus_id_to_reply.get() != self.config.selfdiag_bus_id
        {
            self.forward_message_to_transport_layer(
                message,
                self.bus_id_to_reply.get(),
                notification,
            )
        } else {
            LayerErrorCode::SendFail
        };
        if result == LayerErrorCode::Ok { ReceiveResult::NoError } else { ReceiveResult::Error }
    }
}

// Adapted from transportRouterSimple/test/src/TransportRouterSimpleTest.cpp to the
// routing the Zephyr demo builds (CAN to the diagnostic layer and back, no address
// conversion).
#[cfg(test)]
mod tests {
    extern crate std;

    use std::sync::Mutex;

    use openbsw_timer::NoLock;

    use super::*;
    use crate::layer::tests::TestLayer;

    const SELFDIAG: u8 = 1;
    const CAN_0: u8 = 2;
    const CONFIG: RouterConfig = RouterConfig {
        selfdiag_bus_id: SELFDIAG,
        can_bus_id: CAN_0,
        functional_address: 0xDF,
        max_functional_payload: 6,
    };

    static ROUTER: TransportRouterSimple<NoLock> = TransportRouterSimple::new(&ROUTER, CONFIG);
    /// The router is one static, so the tests run one at a time.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn setup() -> (std::sync::MutexGuard<'static, ()>, &'static TestLayer, &'static TestLayer) {
        let guard = SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        ROUTER.init();
        let can = TestLayer::new(CAN_0);
        let selfdiag = TestLayer::new(SELFDIAG);
        ROUTER.add_transport_layer(can);
        ROUTER.add_transport_layer(selfdiag);
        ROUTER.bus_id_to_reply.set(SELFDIAG);
        for locked in ROUTER.locked.iter().chain(ROUTER.functional_locked.iter()) {
            locked.set(false);
        }
        (guard, can, selfdiag)
    }

    #[test]
    fn request_from_can_response_from_selfdiag_round_trip() {
        let (_serial, can, selfdiag) = setup();
        let request = ROUTER.get_transport_message(CAN_0, 0x00F0, 0x002A, 8, &[]).unwrap();
        request.set_source_address(0x00F0);
        request.set_target_address(0x002A);
        assert_eq!(ROUTER.message_received(CAN_0, request, None), ReceiveResult::NoError);
        assert_eq!(selfdiag.sent.borrow().as_slice(), [(0x00F0, 0x002A)]);
        assert!(can.sent.borrow().is_empty());
        assert!(core::ptr::eq(selfdiag.providing_listener_helper().provider().unwrap(), &ROUTER));
        let response = ROUTER.get_transport_message(SELFDIAG, 0x002A, 0x00F0, 8, &[]).unwrap();
        assert!(!core::ptr::eq(request, response));
        response.set_source_address(0x002A);
        response.set_target_address(0x00F0);
        assert_eq!(ROUTER.message_received(SELFDIAG, response, None), ReceiveResult::NoError);
        assert_eq!(can.sent.borrow().as_slice(), [(0x002A, 0x00F0)]);
        // A layer that fails to send fails the reception.
        can.result.set(LayerErrorCode::QueueFull);
        assert_eq!(ROUTER.message_received(SELFDIAG, response, None), ReceiveResult::Error);
        ROUTER.release_transport_message(request);
        ROUTER.release_transport_message(response);
        ROUTER.remove_transport_layer(can);
        ROUTER.remove_transport_layer(selfdiag);
        assert!(selfdiag.providing_listener_helper().provider().is_none());
        ROUTER.shutdown();
    }

    #[test]
    fn get_transport_message_from_can_functional_address() {
        let (_serial, _can, _selfdiag) = setup();
        let message = ROUTER.get_transport_message(CAN_0, 0x00F3, 0x00DF, 6, &[]).unwrap();
        assert_eq!(message.max_payload_length(), FUNCTIONAL_BUFFER_SIZE as u16);
        // Eight functional buffers, then the large ones.
        let mut taken = std::vec![message];
        for _ in 1..NUM_FUNCTIONAL_BUFFERS {
            taken.push(ROUTER.get_transport_message(CAN_0, 0x00F3, 0x00DF, 6, &[]).unwrap());
        }
        let large = ROUTER.get_transport_message(CAN_0, 0x00F3, 0x00DF, 6, &[]).unwrap();
        assert_eq!(large.max_payload_length(), BUFFER_SIZE as u16);
        // A functional request too long for the small buffers takes a large one.
        let long = ROUTER.get_transport_message(CAN_0, 0x00F3, 0x00DF, 7, &[]).unwrap();
        assert_eq!(long.max_payload_length(), BUFFER_SIZE as u16);
        for message in taken.into_iter().chain([large, long]) {
            ROUTER.release_transport_message(message);
        }
    }

    #[test]
    fn get_transport_message_size_too_large_and_exhausted() {
        let (_serial, _can, _selfdiag) = setup();
        assert_eq!(
            ROUTER.get_transport_message(CAN_0, 0x00F0, 0x002A, BUFFER_SIZE as u16 + 1, &[]),
            Err(ProviderErrorCode::SizeTooLarge)
        );
        let taken: std::vec::Vec<_> = (0..NUM_BUFFERS)
            .map(|_| ROUTER.get_transport_message(CAN_0, 0x00F0, 0x002A, 100, &[]).unwrap())
            .collect();
        assert_eq!(
            ROUTER.get_transport_message(CAN_0, 0x00F0, 0x002A, 100, &[]),
            Err(ProviderErrorCode::NoMsgAvailable)
        );
        ROUTER.release_transport_message(taken[1]);
        assert!(core::ptr::eq(
            ROUTER.get_transport_message(CAN_0, 0x00F0, 0x002A, 100, &[]).unwrap(),
            taken[1]
        ));
        for message in taken {
            ROUTER.release_transport_message(message);
        }
    }

    #[test]
    fn message_received_from_selfdiag_without_reply_target() {
        let (_serial, can, selfdiag) = setup();
        let message = ROUTER.get_transport_message(SELFDIAG, 0x002A, 0x00F0, 8, &[]).unwrap();
        assert_eq!(ROUTER.message_received(SELFDIAG, message, None), ReceiveResult::Error);
        assert!(can.sent.borrow().is_empty());
        // An unknown bus is refused too.
        assert_eq!(ROUTER.message_received(9, message, None), ReceiveResult::Error);
        // A second layer for a bus is refused.
        let other = TestLayer::new(CAN_0);
        ROUTER.add_transport_layer(other);
        assert!(other.providing_listener_helper().provider().is_none());
        ROUTER.release_transport_message(message);
        ROUTER.remove_transport_layer(can);
        ROUTER.remove_transport_layer(selfdiag);
    }
}
