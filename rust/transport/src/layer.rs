// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! A bus's transport layer, ported from `AbstractTransportLayer.h` and `.cpp`.

use core::cell::Cell;

use openbsw_util::format::Arg;
use openbsw_util::log_warn;

use crate::message::{
    ProviderErrorCode, ReceiveResult, TransportMessage, TransportMessageListener,
    TransportMessageProcessedListener, TransportMessageProvider,
};
use crate::{TRANSPORT, bus_name};

/// What a layer's `send` or `init` reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerErrorCode {
    /// Done.
    Ok,
    /// Sending failed.
    SendFail,
    /// The layer's queue is full.
    QueueFull,
    /// The message is not complete.
    MessageIncomplete,
    /// The message is being sent already.
    MessageAlreadyInProgress,
    /// Some other error.
    GeneralError,
}

/// `shutdown` returns this when it completed before returning.
pub const SYNC_SHUTDOWN_COMPLETE: bool = true;

/// Told when a layer's shutdown completes (the C++ `ShutdownDelegate`).
pub trait ShutdownListener: Sync {
    /// `layer` finished shutting down.
    fn shutdown_done(&self, layer: &'static dyn TransportLayer);
}

/// Who a layer tells when its shutdown completes.
pub type ShutdownDelegate = &'static dyn ShutdownListener;

/// A transport layer over one bus: the port of `AbstractTransportLayer`.
pub trait TransportLayer: Sync {
    /// The bus this layer serves.
    fn bus_id(&self) -> u8;

    /// Initialize; the default succeeds.
    fn init(&'static self) -> LayerErrorCode {
        LayerErrorCode::Ok
    }

    /// Shut down; returns whether it completed before returning (the default does), else
    /// `done` is called later.
    fn shutdown(&'static self, done: ShutdownDelegate) -> bool {
        let _ = done;
        SYNC_SHUTDOWN_COMPLETE
    }

    /// Send `message`; `notification` is told when it was processed.
    fn send(
        &'static self,
        message: &'static TransportMessage,
        notification: Option<&'static dyn TransportMessageProcessedListener>,
    ) -> LayerErrorCode;

    /// The hooks the router sets (`fProvidingListenerHelper`).
    fn providing_listener_helper(&self) -> &ProvidingListenerHelper;

    /// The node that links this layer into a router's list.
    fn node(&self) -> &LayerNode;
}

/// The link of a [`TransportLayer`] in a router's list (`etl::bidirectional_link`).
pub struct LayerNode {
    pub(crate) next: Cell<Option<&'static dyn TransportLayer>>,
}

// SAFETY: changed only by the router while layers are added and removed, during the
// lifecycle transitions.
unsafe impl Sync for LayerNode {}

impl LayerNode {
    /// An unlinked node.
    pub const fn new() -> Self {
        Self { next: Cell::new(None) }
    }
}

impl Default for LayerNode {
    fn default() -> Self {
        Self::new()
    }
}

/// A layer's view of the router: forwards to the provider and listener the router set, and
/// warns when there is none (`TransportMessageProvidingListenerHelper`).
pub struct ProvidingListenerHelper {
    bus_id: u8,
    provider: Cell<Option<&'static dyn TransportMessageProvider>>,
    listener: Cell<Option<&'static dyn TransportMessageListener>>,
}

// SAFETY: set by the router while layers are added and removed, read when messages flow.
unsafe impl Sync for ProvidingListenerHelper {}

impl ProvidingListenerHelper {
    /// A helper for the layer of `bus_id`, with no provider or listener.
    pub const fn new(bus_id: u8) -> Self {
        Self { bus_id, provider: Cell::new(None), listener: Cell::new(None) }
    }

    /// Set (or clear) the provider.
    pub fn set_provider(&self, provider: Option<&'static dyn TransportMessageProvider>) {
        self.provider.set(provider);
    }

    /// Set (or clear) the listener.
    pub fn set_listener(&self, listener: Option<&'static dyn TransportMessageListener>) {
        self.listener.set(listener);
    }

    /// The provider, if any.
    pub fn provider(&self) -> Option<&'static dyn TransportMessageProvider> {
        self.provider.get()
    }

    /// The listener, if any.
    pub fn listener(&self) -> Option<&'static dyn TransportMessageListener> {
        self.listener.get()
    }
}

impl TransportMessageProvider for ProvidingListenerHelper {
    fn get_transport_message(
        &self,
        src_bus_id: u8,
        source_address: u16,
        target_address: u16,
        size: u16,
        peek: &[u8],
    ) -> Result<&'static TransportMessage, ProviderErrorCode> {
        match self.provider.get() {
            Some(provider) => provider.get_transport_message(
                src_bus_id,
                source_address,
                target_address,
                size,
                peek,
            ),
            None => {
                log_warn!(
                    TRANSPORT,
                    b"AbstractTransportLayer(%s)::getTransportMessage() with no registered provider!",
                    Arg::Str(Some(bus_name(self.bus_id)))
                );
                Err(ProviderErrorCode::NoMsgAvailable)
            }
        }
    }

    fn release_transport_message(&self, message: &'static TransportMessage) {
        if let Some(provider) = self.provider.get() {
            provider.release_transport_message(message);
        }
    }

    fn dump(&self) {
        if let Some(provider) = self.provider.get() {
            provider.dump();
        }
    }
}

impl TransportMessageListener for ProvidingListenerHelper {
    fn message_received(
        &self,
        source_bus_id: u8,
        message: &'static TransportMessage,
        notification: Option<&'static dyn TransportMessageProcessedListener>,
    ) -> ReceiveResult {
        match self.listener.get() {
            Some(listener) => listener.message_received(source_bus_id, message, notification),
            None => {
                log_warn!(
                    TRANSPORT,
                    b"AbstractTransportLayer(%s)::messageReceived() with no registered listener!",
                    Arg::Str(Some(bus_name(self.bus_id)))
                );
                ReceiveResult::Error
            }
        }
    }
}

// Ported from transport/test/src/AbstractTransportLayerTest.cpp.
#[cfg(test)]
pub(crate) mod tests {
    extern crate std;

    use std::boxed::Box;
    use std::cell::RefCell;
    use std::vec::Vec;

    use super::*;

    /// A layer recording what it was asked to send: the port of the layer mock.
    pub(crate) struct TestLayer {
        bus_id: u8,
        helper: ProvidingListenerHelper,
        node: LayerNode,
        pub(crate) sent: RefCell<Vec<(u16, u16)>>,
        pub(crate) result: Cell<LayerErrorCode>,
    }
    // SAFETY: single-threaded test object.
    unsafe impl Sync for TestLayer {}

    impl TestLayer {
        pub(crate) fn new(bus_id: u8) -> &'static Self {
            Box::leak(Box::new(Self {
                bus_id,
                helper: ProvidingListenerHelper::new(bus_id),
                node: LayerNode::new(),
                sent: RefCell::new(Vec::new()),
                result: Cell::new(LayerErrorCode::Ok),
            }))
        }
    }

    impl TransportLayer for TestLayer {
        fn bus_id(&self) -> u8 {
            self.bus_id
        }
        fn send(
            &'static self,
            message: &'static TransportMessage,
            _notification: Option<&'static dyn TransportMessageProcessedListener>,
        ) -> LayerErrorCode {
            self.sent.borrow_mut().push((message.source_id(), message.target_id()));
            self.result.get()
        }
        fn providing_listener_helper(&self) -> &ProvidingListenerHelper {
            &self.helper
        }
        fn node(&self) -> &LayerNode {
            &self.node
        }
    }

    struct Recorder {
        requests: RefCell<Vec<(u8, u16, u16, u16)>>,
        released: Cell<u32>,
        received: Cell<u32>,
        dumped: Cell<u32>,
    }
    // SAFETY: single-threaded test object.
    unsafe impl Sync for Recorder {}

    static MESSAGE: TransportMessage = TransportMessage::new();

    impl TransportMessageProvider for Recorder {
        fn get_transport_message(
            &self,
            src_bus_id: u8,
            source_address: u16,
            target_address: u16,
            size: u16,
            _peek: &[u8],
        ) -> Result<&'static TransportMessage, ProviderErrorCode> {
            self.requests.borrow_mut().push((src_bus_id, source_address, target_address, size));
            Ok(&MESSAGE)
        }
        fn release_transport_message(&self, _message: &'static TransportMessage) {
            self.released.set(self.released.get() + 1);
        }
        fn dump(&self) {
            self.dumped.set(self.dumped.get() + 1);
        }
    }

    impl TransportMessageListener for Recorder {
        fn message_received(
            &self,
            _source_bus_id: u8,
            _message: &'static TransportMessage,
            _notification: Option<&'static dyn TransportMessageProcessedListener>,
        ) -> ReceiveResult {
            self.received.set(self.received.get() + 1);
            ReceiveResult::NoError
        }
    }

    fn recorder() -> &'static Recorder {
        Box::leak(Box::new(Recorder {
            requests: RefCell::new(Vec::new()),
            released: Cell::new(0),
            received: Cell::new(0),
            dumped: Cell::new(0),
        }))
    }

    #[test]
    fn helper_methods() {
        let recorder = recorder();
        let layer = TestLayer::new(0);
        layer.providing_listener_helper().set_provider(Some(recorder));
        layer.providing_listener_helper().set_listener(Some(recorder));
        let helper = layer.providing_listener_helper();
        assert!(helper.get_transport_message(0, 1, 2, 3, &[]).is_ok());
        assert_eq!(recorder.requests.borrow().as_slice(), [(0, 1, 2, 3)]);
        helper.release_transport_message(&MESSAGE);
        assert_eq!(recorder.released.get(), 1);
        assert_eq!(helper.message_received(0, &MESSAGE, None), ReceiveResult::NoError);
        assert_eq!(recorder.received.get(), 1);
        helper.dump();
        assert_eq!(recorder.dumped.get(), 1);
    }

    #[test]
    fn helper_methods_without_provider() {
        let layer = TestLayer::new(0);
        let helper = layer.providing_listener_helper();
        assert_eq!(
            helper.get_transport_message(0, 1, 2, 3, &[]),
            Err(ProviderErrorCode::NoMsgAvailable)
        );
        helper.release_transport_message(&MESSAGE);
        assert_eq!(helper.message_received(0, &MESSAGE, None), ReceiveResult::Error);
        helper.dump();
    }

    #[test]
    fn default_implementations() {
        let layer = TestLayer::new(0);
        assert_eq!(layer.init(), LayerErrorCode::Ok);
        struct Done;
        impl ShutdownListener for Done {
            fn shutdown_done(&self, _layer: &'static dyn TransportLayer) {}
        }
        static DONE: Done = Done;
        assert!(layer.shutdown(&DONE));
    }
}
