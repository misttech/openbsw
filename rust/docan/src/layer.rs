// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The transport layer the router sees, ported from `transport/DoCanTransportLayer.h`,
//! `DoCanTransportLayerConfig.h` and `DoCanTransportLayerContainer.h`.

use core::cell::Cell;

use openbsw_async::{ContextType, QueueNode, Runnable};
use openbsw_timer::Lock;
use openbsw_transport::{
    LayerErrorCode, LayerNode, ProvidingListenerHelper, ShutdownDelegate, ShutdownListener,
    TransportLayer, TransportMessage, TransportMessageProcessedListener,
};
use openbsw_util::logger::LoggerComponent;

use crate::addressing::AddressConverter;
use crate::common::{
    Address, Connection, DoCanParameters, FlowStatus, FrameIndex, FrameSize, MessageSize,
};
use crate::receiver::{DoCanReceiver, ReceiverPool, ReceiverSlots};
use crate::transceiver::{FrameReceiver, PhysicalTransceiver, TickGenerator};
use crate::transmitter::{DoCanTransmitter, TransmitterPool, TransmitterSlots};

/// The parameters and the pools of one or more layers: the port of
/// `declare::DoCanTransportLayerConfig<…, RxCount, TxCount, MaxFrameSize>`, with `RX`
/// receiver and `TX` transmitter slots.
pub struct DoCanTransportLayerConfig<const RX: usize, const TX: usize> {
    receiver_pool: ReceiverPool<RX>,
    transmitter_pool: TransmitterPool<TX>,
    parameters: &'static DoCanParameters,
}

impl<const RX: usize, const TX: usize> DoCanTransportLayerConfig<RX, TX> {
    /// A config over `parameters`, with empty pools.
    pub const fn new(parameters: &'static DoCanParameters) -> Self {
        Self {
            receiver_pool: ReceiverPool::new(),
            transmitter_pool: TransmitterPool::new(),
            parameters,
        }
    }

    /// The receiver pool.
    pub fn message_receiver_pool(&self) -> &dyn ReceiverSlots {
        &self.receiver_pool
    }

    /// The transmitter pool.
    pub fn message_transmitter_pool(&self) -> &dyn TransmitterSlots {
        &self.transmitter_pool
    }

    /// The parameters.
    pub fn parameters(&self) -> &'static DoCanParameters {
        self.parameters
    }
}

/// The runnable that runs a layer's shutdown on its context.
struct ProcessShutdown<L: Lock + 'static> {
    owner: &'static DoCanTransportLayer<L>,
    node: QueueNode<dyn Runnable>,
}

impl<L: Lock + 'static> Runnable for ProcessShutdown<L> {
    fn execute(&self) {
        self.owner.process_shutdown();
    }

    fn node(&self) -> &QueueNode<dyn Runnable> {
        &self.node
    }
}

/// ISO 15765-2 over one CAN bus: the port of `DoCanTransportLayer`.
pub struct DoCanTransportLayer<L: Lock + 'static> {
    bus_id: u8,
    helper: ProvidingListenerHelper,
    node: LayerNode,
    transceiver: &'static dyn PhysicalTransceiver,
    receiver: DoCanReceiver<L>,
    transmitter: DoCanTransmitter<L>,
    process_shutdown: ProcessShutdown<L>,
    shutdown_delegate: Cell<Option<ShutdownDelegate>>,
    context: ContextType,
}

// SAFETY: the shutdown delegate is set by `shutdown` and read by the shutdown runnable it
// queues, one after the other.
unsafe impl<L: Lock + 'static> Sync for DoCanTransportLayer<L> {}

impl<L: Lock + 'static> DoCanTransportLayer<L> {
    /// A layer on `bus_id`, running on `context`, given its own `'static` address: receiving
    /// through and sending with `transceiver`, logging to `logger`.
    #[expect(clippy::too_many_arguments, reason = "the C++ constructor's parameters")]
    pub const fn new<const RX: usize, const TX: usize>(
        this: &'static Self,
        bus_id: u8,
        context: ContextType,
        address_converter: &'static dyn AddressConverter,
        transceiver: &'static dyn PhysicalTransceiver,
        tick_generator: &'static dyn TickGenerator,
        config: &'static DoCanTransportLayerConfig<RX, TX>,
        logger: &'static LoggerComponent,
    ) -> Self {
        Self {
            bus_id,
            helper: ProvidingListenerHelper::new(bus_id),
            node: LayerNode::new(),
            transceiver,
            receiver: DoCanReceiver::new(
                &this.receiver,
                bus_id,
                context,
                &this.helper,
                transceiver,
                &config.receiver_pool,
                address_converter,
                config.parameters,
                logger,
            ),
            transmitter: DoCanTransmitter::new(
                &this.transmitter,
                bus_id,
                context,
                transceiver,
                tick_generator,
                &config.transmitter_pool,
                address_converter,
                config.parameters,
                logger,
            ),
            process_shutdown: ProcessShutdown { owner: this, node: QueueNode::new() },
            shutdown_delegate: Cell::new(None),
            context,
        }
    }

    /// Run the timeouts of both sides that are due at `now_us`.
    pub fn cyclic_task(&self, now_us: u32) {
        self.transmitter.cyclic_task(now_us);
        self.receiver.cyclic_task(now_us);
    }

    /// Send the consecutive frames due at `now_us`; returns whether more ticks are needed.
    pub fn tick(&self, now_us: u32) -> bool {
        self.transmitter.cyclic_task(now_us);
        self.transmitter.is_sending_consecutive_frames()
    }

    fn process_shutdown(&'static self) {
        self.transmitter.shutdown();
        self.receiver.shutdown();
        self.transceiver.shutdown();
        if let Some(delegate) = self.shutdown_delegate.get() {
            delegate.shutdown_done(self);
        }
    }
}

impl<L: Lock + 'static> TransportLayer for DoCanTransportLayer<L> {
    fn bus_id(&self) -> u8 {
        self.bus_id
    }

    fn init(&'static self) -> LayerErrorCode {
        self.transceiver.init(self);
        self.transmitter.init();
        self.receiver.init();
        LayerErrorCode::Ok
    }

    fn shutdown(&'static self, done: ShutdownDelegate) -> bool {
        self.shutdown_delegate.set(Some(done));
        openbsw_async::execute(self.context, &self.process_shutdown);
        false
    }

    fn send(
        &'static self,
        message: &'static TransportMessage,
        notification: Option<&'static dyn TransportMessageProcessedListener>,
    ) -> LayerErrorCode {
        self.transmitter.send(message, notification)
    }

    fn providing_listener_helper(&self) -> &ProvidingListenerHelper {
        &self.helper
    }

    fn node(&self) -> &LayerNode {
        &self.node
    }
}

impl<L: Lock + 'static> FrameReceiver for DoCanTransportLayer<L> {
    fn first_data_frame_received(
        &self,
        connection: &Connection,
        message_size: MessageSize,
        frame_count: FrameIndex,
        consecutive_frame_data_size: FrameSize,
        data: &[u8],
    ) {
        self.receiver.first_data_frame_received(
            connection,
            message_size,
            frame_count,
            consecutive_frame_data_size,
            data,
        );
    }

    fn consecutive_data_frame_received(
        &self,
        reception_address: Address,
        sequence_number: u8,
        data: &[u8],
    ) {
        self.receiver.consecutive_data_frame_received(reception_address, sequence_number, data);
    }

    fn flow_control_frame_received(
        &self,
        reception_address: Address,
        flow_status: FlowStatus,
        block_size: u8,
        encoded_min_separation_time: u8,
    ) {
        self.transmitter.flow_control_frame_received(
            reception_address,
            flow_status,
            block_size,
            encoded_min_separation_time,
        );
    }
}

/// Called once every layer of a container shut down.
pub type ShutdownCallback = &'static (dyn Fn() + Sync);

/// The layers of one system, driven together: the port of `DoCanTransportLayerContainer`.
///
/// The C++ container owns its layers and builds them in place; here each layer is a
/// `static` of its own and the container lists them.
pub struct DoCanTransportLayerContainer<L: Lock + 'static> {
    layers: &'static [&'static DoCanTransportLayer<L>],
    shutdown_callback: Cell<Option<ShutdownCallback>>,
    shutdown_pending_count: Cell<u8>,
}

// SAFETY: the callback is set by `shutdown` and read when the last layer reports; the count
// changes under `L`.
unsafe impl<L: Lock + 'static> Sync for DoCanTransportLayerContainer<L> {}

impl<L: Lock + 'static> DoCanTransportLayerContainer<L> {
    /// A container of `layers`.
    pub const fn new(layers: &'static [&'static DoCanTransportLayer<L>]) -> Self {
        Self { layers, shutdown_callback: Cell::new(None), shutdown_pending_count: Cell::new(0) }
    }

    /// The layers.
    pub fn transport_layers(&self) -> &'static [&'static DoCanTransportLayer<L>] {
        self.layers
    }

    /// Initialize every layer.
    pub fn init(&self) {
        for layer in self.layers {
            let _ = layer.init();
        }
    }

    /// Shut every layer down; `shutdown_callback` is called once all have.
    pub fn shutdown(&'static self, shutdown_callback: ShutdownCallback) {
        self.shutdown_pending_count.set(1);
        self.shutdown_callback.set(Some(shutdown_callback));
        for layer in self.layers {
            {
                assert!(
                    self.shutdown_pending_count.get() != u8::MAX,
                    "pending count must not wrap"
                );
                let _lock = L::lock();
                self.shutdown_pending_count.set(self.shutdown_pending_count.get() + 1);
            }
            let _ = layer.shutdown(self);
        }
        self.single_shutdown_done();
    }

    /// Run every layer's cyclic task.
    pub fn cyclic_task(&self, now_us: u32) {
        for layer in self.layers {
            layer.cyclic_task(now_us);
        }
    }

    /// Tick every layer; returns whether any needs more ticks.
    pub fn tick(&self, now_us: u32) -> bool {
        let mut result = false;
        for layer in self.layers {
            if layer.tick(now_us) {
                result = true;
            }
        }
        result
    }

    fn single_shutdown_done(&self) {
        let done = {
            assert!(self.shutdown_pending_count.get() != 0, "pending count must not wrap");
            let _lock = L::lock();
            self.shutdown_pending_count.set(self.shutdown_pending_count.get() - 1);
            self.shutdown_pending_count.get() == 0
        };
        if done && let Some(callback) = self.shutdown_callback.get() {
            callback();
        }
    }
}

impl<L: Lock + 'static> ShutdownListener for DoCanTransportLayerContainer<L> {
    fn shutdown_done(&self, _layer: &'static dyn TransportLayer) {
        self.single_shutdown_done();
    }
}

// Ported from docan/test/src/docan/transport/DoCanTransportLayerTest.cpp and
// DoCanTransportLayerContainerTest.cpp.
#[cfg(test)]
mod tests {
    extern crate std;

    use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
    use std::boxed::Box;
    use std::sync::Mutex;
    use std::vec::Vec;

    use openbsw_async::mock::MockAsync;
    use openbsw_timer::NoLock;
    use openbsw_transport::{
        ProcessingResult, ProviderErrorCode, ReceiveResult, TransportMessageListener,
        TransportMessageProvider,
    };

    use super::*;
    use crate::codec::{DefaultFrameSizeMapper, FrameCodec, FrameCodecConfig, presets};
    use crate::common::{DataLinkAddressPair, JobHandle, TransportAddressPair};
    use crate::transceiver::{
        DataFrameTransmitter, DataFrameTransmitterCallback, FlowControlFrameTransmitter, SendResult,
    };

    /// The layers and mocks are process-wide statics here, so the tests run one at a time.
    static SERIAL: Mutex<()> = Mutex::new(());

    const BUS_ID: u8 = 0xFF;
    const BUS_ID2: u8 = 0xFE;
    const CONTEXT: ContextType = 1;

    static NOW_US: AtomicU32 = AtomicU32::new(0);
    fn now_us() -> u32 {
        NOW_US.load(Ordering::Relaxed)
    }
    static PARAMETERS: DoCanParameters =
        DoCanParameters::new(&now_us, 200, 3543, 1232, 3442, 98, 153, 0, 0);
    static CONFIG: DoCanTransportLayerConfig<2, 3> = DoCanTransportLayerConfig::new(&PARAMETERS);
    static MOCK_ASYNC: MockAsync<2> = MockAsync::new([b"main" as &[u8], b"docan"]);
    static CODEC_CONFIG: FrameCodecConfig = presets::OPTIMIZED_CLASSIC;
    static MAPPER: DefaultFrameSizeMapper = DefaultFrameSizeMapper;
    static CODEC: FrameCodec = FrameCodec::new(&CODEC_CONFIG, &MAPPER);
    static LOGGER: LoggerComponent = LoggerComponent::new();

    /// `DoCanAddressConverterMock`: answers the next transmission lookup as scripted and
    /// records the addresses it was asked to format.
    struct MockConverter {
        transmission: Mutex<Option<(&'static FrameCodec, DataLinkAddressPair)>>,
        asked: Mutex<Vec<TransportAddressPair>>,
        formatted: Mutex<Vec<Address>>,
    }

    impl AddressConverter for MockConverter {
        fn transmission_parameters(
            &self,
            pair: &TransportAddressPair,
        ) -> Option<(&'static FrameCodec, DataLinkAddressPair)> {
            self.asked.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(*pair);
            self.transmission.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take()
        }

        fn reception_parameters(
            &self,
            _reception_address: Address,
        ) -> Option<(&'static FrameCodec, TransportAddressPair, Address)> {
            None
        }

        fn format_data_link_address<'b>(&self, address: Address, buffer: &'b mut [u8]) -> &'b [u8] {
            self.formatted.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(address);
            buffer[..3].copy_from_slice(b"abc");
            &buffer[..3]
        }
    }

    static CONVERTER: MockConverter = MockConverter {
        transmission: Mutex::new(None),
        asked: Mutex::new(Vec::new()),
        formatted: Mutex::new(Vec::new()),
    };

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct SendCall {
        job_handle: JobHandle,
        address: Address,
        first: FrameIndex,
        last: FrameIndex,
        cf_size: FrameSize,
        data: Vec<u8>,
    }

    /// `DoCanPhysicalTransceiverMock`: records what the layer asks of the bus.
    struct MockTransceiver {
        receiver: Mutex<Option<&'static dyn FrameReceiver>>,
        callback: Mutex<Option<&'static dyn DataFrameTransmitterCallback>>,
        sends: Mutex<Vec<SendCall>>,
        flow_controls: Mutex<Vec<(Address, FlowStatus, u8, u8)>>,
        shutdowns: AtomicUsize,
    }

    impl MockTransceiver {
        const fn new() -> Self {
            Self {
                receiver: Mutex::new(None),
                callback: Mutex::new(None),
                sends: Mutex::new(Vec::new()),
                flow_controls: Mutex::new(Vec::new()),
                shutdowns: AtomicUsize::new(0),
            }
        }

        fn receiver(&self) -> &'static dyn FrameReceiver {
            self.receiver.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).expect("init")
        }

        fn callback(&self) -> &'static dyn DataFrameTransmitterCallback {
            self.callback.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).expect("a send")
        }

        fn take_sends(&self) -> Vec<SendCall> {
            core::mem::take(
                &mut *self.sends.lock().unwrap_or_else(|poisoned| poisoned.into_inner()),
            )
        }
    }

    impl DataFrameTransmitter for MockTransceiver {
        fn start_send_data_frames(
            &self,
            _codec: &FrameCodec,
            callback: &'static dyn DataFrameTransmitterCallback,
            job_handle: JobHandle,
            transmission_address: Address,
            first_frame_index: FrameIndex,
            last_frame_index: FrameIndex,
            consecutive_frame_data_size: FrameSize,
            data: &[Cell<u8>],
        ) -> SendResult {
            *self.callback.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(callback);
            self.sends.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(SendCall {
                job_handle,
                address: transmission_address,
                first: first_frame_index,
                last: last_frame_index,
                cf_size: consecutive_frame_data_size,
                data: data.iter().map(Cell::get).collect(),
            });
            SendResult::QueuedFull
        }

        fn cancel_send_data_frames(
            &self,
            _callback: &'static dyn DataFrameTransmitterCallback,
            _job_handle: JobHandle,
        ) {
        }
    }

    impl FlowControlFrameTransmitter for MockTransceiver {
        fn send_flow_control(
            &self,
            _codec: &FrameCodec,
            transmission_address: Address,
            flow_status: FlowStatus,
            block_size: u8,
            encoded_min_separation_time: u8,
        ) -> bool {
            self.flow_controls.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push((
                transmission_address,
                flow_status,
                block_size,
                encoded_min_separation_time,
            ));
            true
        }
    }

    impl PhysicalTransceiver for MockTransceiver {
        fn init(&self, receiver: &'static dyn FrameReceiver) {
            *self.receiver.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(receiver);
        }

        fn shutdown(&self) {
            self.shutdowns.fetch_add(1, Ordering::Relaxed);
        }
    }

    static TRANSCEIVER1: MockTransceiver = MockTransceiver::new();
    static TRANSCEIVER2: MockTransceiver = MockTransceiver::new();

    struct MockTickGenerator(AtomicUsize);

    impl TickGenerator for MockTickGenerator {
        fn tick_needed(&self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    static TICK_GENERATOR: MockTickGenerator = MockTickGenerator(AtomicUsize::new(0));

    /// `TransportMessageProvidingListenerMock`: hands out the scripted message and keeps the
    /// notification listener of each received message.
    struct MockProvidingListener {
        next: Mutex<Option<Result<&'static TransportMessage, ProviderErrorCode>>>,
        requests: Mutex<Vec<(u8, u16, u16, u16)>>,
        received: Mutex<
            Vec<(u8, &'static TransportMessage, &'static dyn TransportMessageProcessedListener)>,
        >,
        released: Mutex<Vec<&'static TransportMessage>>,
    }

    impl TransportMessageProvider for MockProvidingListener {
        fn get_transport_message(
            &self,
            src_bus_id: u8,
            source_address: u16,
            target_address: u16,
            size: u16,
            _peek: &[u8],
        ) -> Result<&'static TransportMessage, ProviderErrorCode> {
            self.requests.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push((
                src_bus_id,
                source_address,
                target_address,
                size,
            ));
            self.next
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
                .unwrap_or(Err(ProviderErrorCode::NoMsgAvailable))
        }

        fn release_transport_message(&self, message: &'static TransportMessage) {
            self.released.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(message);
        }
    }

    impl TransportMessageListener for MockProvidingListener {
        fn message_received(
            &self,
            source_bus_id: u8,
            message: &'static TransportMessage,
            notification: Option<&'static dyn TransportMessageProcessedListener>,
        ) -> ReceiveResult {
            self.received.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push((
                source_bus_id,
                message,
                notification.expect("a listener"),
            ));
            ReceiveResult::NoError
        }
    }

    static PROVIDING_LISTENER: MockProvidingListener = MockProvidingListener {
        next: Mutex::new(None),
        requests: Mutex::new(Vec::new()),
        received: Mutex::new(Vec::new()),
        released: Mutex::new(Vec::new()),
    };

    struct MockProcessedListener(Mutex<Vec<(&'static TransportMessage, ProcessingResult)>>);

    impl TransportMessageProcessedListener for MockProcessedListener {
        fn transport_message_processed(
            &self,
            message: &'static TransportMessage,
            result: ProcessingResult,
        ) {
            self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push((message, result));
        }
    }

    static PROCESSED_LISTENER: MockProcessedListener =
        MockProcessedListener(Mutex::new(Vec::new()));

    struct MockShutdownListener(AtomicUsize);

    impl ShutdownListener for MockShutdownListener {
        fn shutdown_done(&self, layer: &'static dyn TransportLayer) {
            assert_eq!(layer.bus_id(), BUS_ID);
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    static SHUTDOWN_LISTENER: MockShutdownListener = MockShutdownListener(AtomicUsize::new(0));

    static LAYER1: DoCanTransportLayer<NoLock> = DoCanTransportLayer::new(
        &LAYER1,
        BUS_ID,
        CONTEXT,
        &CONVERTER,
        &TRANSCEIVER1,
        &TICK_GENERATOR,
        &CONFIG,
        &LOGGER,
    );
    static LAYER2: DoCanTransportLayer<NoLock> = DoCanTransportLayer::new(
        &LAYER2,
        BUS_ID2,
        CONTEXT,
        &CONVERTER,
        &TRANSCEIVER2,
        &TICK_GENERATOR,
        &CONFIG,
        &LOGGER,
    );
    static LAYERS: [&DoCanTransportLayer<NoLock>; 2] = [&LAYER1, &LAYER2];
    static CONTAINER: DoCanTransportLayerContainer<NoLock> =
        DoCanTransportLayerContainer::new(&LAYERS);
    static CONTAINER_SHUTDOWNS: AtomicUsize = AtomicUsize::new(0);
    fn container_shutdown_done() {
        CONTAINER_SHUTDOWNS.fetch_add(1, Ordering::Relaxed);
    }

    fn setup() -> std::sync::MutexGuard<'static, ()> {
        let guard = SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        openbsw_async::set_binding(&MOCK_ASYNC);
        NOW_US.store(0, Ordering::Relaxed);
        LAYER1.providing_listener_helper().set_provider(Some(&PROVIDING_LISTENER));
        LAYER1.providing_listener_helper().set_listener(Some(&PROVIDING_LISTENER));
        CONVERTER.formatted.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clear();
        CONVERTER.asked.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clear();
        PROVIDING_LISTENER.requests.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clear();
        PROVIDING_LISTENER.received.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clear();
        PROVIDING_LISTENER.released.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clear();
        PROCESSED_LISTENER.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clear();
        TRANSCEIVER1.take_sends();
        TRANSCEIVER1.flow_controls.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clear();
        guard
    }

    fn message(capacity: usize) -> &'static TransportMessage {
        let buffer: &'static [Cell<u8>] =
            Box::leak((0..capacity).map(|_| Cell::new(0)).collect::<Vec<_>>().into_boxed_slice());
        Box::leak(Box::new(TransportMessage::with_buffer(buffer)))
    }

    fn payload(message: &TransportMessage) -> Vec<u8> {
        message.payload().iter().map(Cell::get).collect()
    }

    fn execute() {
        MOCK_ASYNC.run_runnables(CONTEXT);
    }

    #[test]
    fn transport_message_reception_lifecycle() {
        let _guard = setup();
        assert_eq!(LAYER1.init(), LayerErrorCode::Ok);
        let frame_receiver = TRANSCEIVER1.receiver();
        // Receive a single frame message without immediate allocation success.
        {
            let transport_message = message(4);
            let data = [0x12, 0x34, 0x56, 0x78];
            let connection = Connection::new(
                &CODEC,
                DataLinkAddressPair::new(0x1234857, 0x7654321),
                TransportAddressPair::new(0x35, 0x69),
            );
            frame_receiver.first_data_frame_received(&connection, 4, 1, 0, &data);
            assert_eq!(
                PROVIDING_LISTENER
                    .requests
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .as_slice(),
                &[(BUS_ID, 0x35, 0x69, 4)]
            );
            *PROVIDING_LISTENER.next.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some(Ok(transport_message));
            LAYER1.cyclic_task(now_us());
            NOW_US.fetch_add(200_000, Ordering::Relaxed);
            LAYER1.cyclic_task(now_us());
            assert_eq!(
                PROVIDING_LISTENER
                    .requests
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .len(),
                2
            );
            let (bus, received, notification) = PROVIDING_LISTENER
                .received
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(0);
            assert_eq!(bus, BUS_ID);
            assert!(core::ptr::eq(received, transport_message));
            assert_eq!(transport_message.source_id(), 0x35);
            assert_eq!(transport_message.target_id(), 0x69);
            assert_eq!(payload(transport_message), data);
            execute();
            notification.transport_message_processed(transport_message, ProcessingResult::NoError);
            execute();
            assert!(core::ptr::eq(
                PROVIDING_LISTENER
                    .released
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(0),
                transport_message
            ));
        }
        // Receive a segmented message.
        {
            let transport_message = message(7);
            let data = [0x12, 0x34, 0x56, 0x78, 0x91, 0x73, 0x85];
            let connection = Connection::new(
                &CODEC,
                DataLinkAddressPair::new(0x13348447, 0x76333211),
                TransportAddressPair::new(0x37, 0x96),
            );
            *PROVIDING_LISTENER.next.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some(Ok(transport_message));
            frame_receiver.first_data_frame_received(&connection, 7, 2, 7, &data[..6]);
            assert_eq!(
                TRANSCEIVER1
                    .flow_controls
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .as_slice(),
                &[(0x76333211, FlowStatus::Cts, 0x00, 0x00)]
            );
            TRANSCEIVER1
                .flow_controls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clear();
            frame_receiver.consecutive_data_frame_received(0x13348447, 1, &data[6..]);
            let (_, received, notification) = PROVIDING_LISTENER
                .received
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(0);
            assert!(core::ptr::eq(received, transport_message));
            assert_eq!(transport_message.source_id(), 0x37);
            assert_eq!(transport_message.target_id(), 0x96);
            assert_eq!(payload(transport_message), data);
            execute();
            notification.transport_message_processed(transport_message, ProcessingResult::NoError);
            execute();
            assert!(core::ptr::eq(
                PROVIDING_LISTENER
                    .released
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(0),
                transport_message
            ));
        }
        // Receive a segmented message with the escape sequence.
        {
            const MESSAGE_SIZE: usize = 4999;
            const FRAME_SIZE: usize = 7;
            const FRAMES: usize = MESSAGE_SIZE / FRAME_SIZE + 1;
            let transport_message = message(MESSAGE_SIZE);
            let data: Vec<u8> = (0..MESSAGE_SIZE).map(|i| i as u8).collect();
            let connection = Connection::new(
                &CODEC,
                DataLinkAddressPair::new(0x13348447, 0x76333211),
                TransportAddressPair::new(0x37, 0x96),
            );
            *PROVIDING_LISTENER.next.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some(Ok(transport_message));
            frame_receiver.first_data_frame_received(
                &connection,
                MESSAGE_SIZE as MessageSize,
                FRAMES as FrameIndex,
                FRAME_SIZE as FrameSize,
                &data[..FRAME_SIZE],
            );
            assert_eq!(
                PROVIDING_LISTENER
                    .requests
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .last(),
                Some(&(BUS_ID, 0x37, 0x96, MESSAGE_SIZE as u16))
            );
            assert_eq!(
                TRANSCEIVER1
                    .flow_controls
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .as_slice(),
                &[(0x76333211, FlowStatus::Cts, 0x00, 0x00)]
            );
            TRANSCEIVER1
                .flow_controls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clear();
            let mut slice = &data[FRAME_SIZE..];
            for i in 1..FRAMES - 1 {
                frame_receiver.consecutive_data_frame_received(
                    0x13348447,
                    (i & 0x0F) as u8,
                    &slice[..FRAME_SIZE],
                );
                slice = &slice[FRAME_SIZE..];
            }
            assert!(slice.len() < FRAME_SIZE);
            frame_receiver.consecutive_data_frame_received(
                0x13348447,
                ((FRAMES - 1) & 0x0F) as u8,
                slice,
            );
            let (_, received, notification) = PROVIDING_LISTENER
                .received
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(0);
            assert!(core::ptr::eq(received, transport_message));
            assert_eq!(transport_message.source_id(), 0x37);
            assert_eq!(transport_message.target_id(), 0x96);
            assert_eq!(payload(transport_message), data);
            execute();
            notification.transport_message_processed(transport_message, ProcessingResult::NoError);
            execute();
            assert!(core::ptr::eq(
                PROVIDING_LISTENER
                    .released
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(0),
                transport_message
            ));
        }
        // Receive an unexpected consecutive frame.
        {
            let data = [0x12, 0x34, 0x56, 0x78];
            frame_receiver.consecutive_data_frame_received(0x1234857, 7, &data);
            assert_eq!(
                CONVERTER
                    .formatted
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .as_slice(),
                &[0x1234857]
            );
        }
        let shutdowns = TRANSCEIVER1.shutdowns.load(Ordering::Relaxed);
        let done = SHUTDOWN_LISTENER.0.load(Ordering::Relaxed);
        assert!(!LAYER1.shutdown(&SHUTDOWN_LISTENER));
        assert_eq!(TRANSCEIVER1.shutdowns.load(Ordering::Relaxed), shutdowns);
        execute();
        assert_eq!(TRANSCEIVER1.shutdowns.load(Ordering::Relaxed), shutdowns + 1);
        assert_eq!(SHUTDOWN_LISTENER.0.load(Ordering::Relaxed), done + 1);
    }

    #[test]
    fn transport_message_transmission_lifecycle() {
        let _guard = setup();
        assert_eq!(LAYER1.init(), LayerErrorCode::Ok);
        let frame_receiver = TRANSCEIVER1.receiver();
        // Send a segmented message.
        {
            let transport_message = message(30);
            let data = [
                0x12, 0x34, 0x56, 0x78, 0x19, 0x38, 0x9a, 0x5f, 0x14, 0x91, 0xa3, 0x57, 0x89, 0x99,
            ];
            transport_message.set_source_address(0x56);
            transport_message.set_target_address(0x64);
            transport_message.append(&data);
            transport_message.set_payload_length(data.len() as u16);
            *CONVERTER.transmission.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some((&CODEC, DataLinkAddressPair::new(0x1235689, 0x986321)));
            assert_eq!(
                LAYER1.send(transport_message, Some(&PROCESSED_LISTENER)),
                LayerErrorCode::Ok
            );
            assert_eq!(
                CONVERTER.asked.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).as_slice(),
                &[TransportAddressPair::new(0x56, 0x64)]
            );
            assert!(TRANSCEIVER1.take_sends().is_empty());
            execute();
            let sends = TRANSCEIVER1.take_sends();
            assert_eq!(sends.len(), 1);
            let job_handle = sends[0].job_handle;
            assert_eq!(
                sends[0],
                SendCall {
                    job_handle,
                    address: 0x986321,
                    first: 0,
                    last: 1,
                    cf_size: 7,
                    data: data.to_vec()
                }
            );
            let callback = TRANSCEIVER1.callback();
            callback.data_frames_sent(job_handle, 1, 6);
            // Expect flow control.
            frame_receiver.flow_control_frame_received(0x1235689, FlowStatus::Cts, 0, 1);
            assert_eq!(
                TRANSCEIVER1.take_sends(),
                &[SendCall {
                    job_handle,
                    address: 0x986321,
                    first: 1,
                    last: 2,
                    cf_size: 7,
                    data: data[6..].to_vec()
                }]
            );
            let ticks = TICK_GENERATOR.0.load(Ordering::Relaxed);
            callback.data_frames_sent(job_handle, 1, 7);
            assert_eq!(TICK_GENERATOR.0.load(Ordering::Relaxed), ticks + 1);
            // Tick received.
            NOW_US.store(1000, Ordering::Relaxed);
            LAYER1.tick(now_us());
            assert_eq!(
                TRANSCEIVER1.take_sends(),
                &[SendCall {
                    job_handle,
                    address: 0x986321,
                    first: 2,
                    last: 3,
                    cf_size: 7,
                    data: data[13..].to_vec()
                }]
            );
            callback.data_frames_sent(job_handle, 1, 1);
            // The finished transmitter is removed, and its listener told, on the context.
            assert!(
                PROCESSED_LISTENER
                    .0
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .is_empty()
            );
            execute();
            let processed = PROCESSED_LISTENER
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(0);
            assert!(core::ptr::eq(processed.0, transport_message));
            assert_eq!(processed.1, ProcessingResult::NoError);
        }
        // Send to an unknown address pair.
        {
            let transport_message = message(10);
            let data = [0x12, 0x34, 0x56, 0x78, 0x19];
            transport_message.set_source_address(0x56);
            transport_message.set_target_address(0x64);
            transport_message.append(&data);
            transport_message.set_payload_length(data.len() as u16);
            assert_eq!(
                LAYER1.send(transport_message, Some(&PROCESSED_LISTENER)),
                LayerErrorCode::SendFail
            );
        }
        // Receive an unexpected flow control frame.
        {
            frame_receiver.flow_control_frame_received(0x1235689, FlowStatus::Cts, 0, 1);
            assert_eq!(
                CONVERTER
                    .formatted
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .as_slice(),
                &[0x1235689]
            );
        }
        let shutdowns = TRANSCEIVER1.shutdowns.load(Ordering::Relaxed);
        let done = SHUTDOWN_LISTENER.0.load(Ordering::Relaxed);
        assert!(!LAYER1.shutdown(&SHUTDOWN_LISTENER));
        execute();
        assert_eq!(TRANSCEIVER1.shutdowns.load(Ordering::Relaxed), shutdowns + 1);
        assert_eq!(SHUTDOWN_LISTENER.0.load(Ordering::Relaxed), done + 1);
    }

    #[test]
    fn container_drives_its_layers() {
        let _guard = setup();
        assert_eq!(CONTAINER.transport_layers().len(), 2);
        assert!(core::ptr::eq(CONTAINER.transport_layers()[0], &LAYER1));
        assert!(core::ptr::eq(CONTAINER.transport_layers()[1], &LAYER2));
        CONTAINER.init();
        let frame_receiver1 = TRANSCEIVER1.receiver();
        TRANSCEIVER2.receiver();
        // Send a segmented message needing ticks.
        {
            let data = [
                0x01, 0x02, 0x03, 0x13, 0x24, 0x56, 0x78, 0x91, 0xa1, 0x94, 0x10, 0x19, 0x88, 0x77,
                0x66,
            ];
            let transport_message = message(20);
            transport_message.set_source_address(0x46);
            transport_message.set_target_address(0x89);
            transport_message.append(&data);
            transport_message.set_payload_length(data.len() as u16);
            *CONVERTER.transmission.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some((&CODEC, DataLinkAddressPair::new(0x12345678, 0x87654321)));
            assert_eq!(
                CONTAINER.transport_layers()[0].send(transport_message, Some(&PROCESSED_LISTENER)),
                LayerErrorCode::Ok
            );
            execute();
            let sends = TRANSCEIVER1.take_sends();
            assert_eq!(sends.len(), 1);
            let job_handle = sends[0].job_handle;
            assert_eq!(
                (sends[0].address, sends[0].first, sends[0].last, sends[0].cf_size),
                (0x87654321, 0, 1, 7)
            );
            let callback = TRANSCEIVER1.callback();
            callback.data_frames_sent(job_handle, 1, 6);
            frame_receiver1.flow_control_frame_received(0x12345678, FlowStatus::Cts, 0x00, 0x1);
            let sends = TRANSCEIVER1.take_sends();
            assert_eq!((sends[0].job_handle, sends[0].first, sends[0].last), (job_handle, 1, 2));
            assert!(!CONTAINER.tick(now_us()));
            let ticks = TICK_GENERATOR.0.load(Ordering::Relaxed);
            callback.data_frames_sent(job_handle, 1, 7);
            assert_eq!(TICK_GENERATOR.0.load(Ordering::Relaxed), ticks + 1);
            // True: consecutive frames are being paced, which needs high-frequency ticks.
            assert!(CONTAINER.tick(now_us()));
            NOW_US.store(1000, Ordering::Relaxed);
            // The single consecutive frame left goes out, so no more ticks are needed.
            assert!(!CONTAINER.tick(now_us()));
            let sends = TRANSCEIVER1.take_sends();
            assert_eq!((sends[0].job_handle, sends[0].first, sends[0].last), (job_handle, 2, 3));
            callback.data_frames_sent(job_handle, 1, 2);
            execute();
            let processed = PROCESSED_LISTENER
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(0);
            assert!(core::ptr::eq(processed.0, transport_message));
            assert_eq!(processed.1, ProcessingResult::NoError);
        }
        CONTAINER.cyclic_task(now_us());
        // Shutdown.
        let shutdowns = (
            TRANSCEIVER1.shutdowns.load(Ordering::Relaxed),
            TRANSCEIVER2.shutdowns.load(Ordering::Relaxed),
        );
        let done = CONTAINER_SHUTDOWNS.load(Ordering::Relaxed);
        CONTAINER.shutdown(&container_shutdown_done);
        assert_eq!(CONTAINER_SHUTDOWNS.load(Ordering::Relaxed), done);
        execute();
        assert_eq!(TRANSCEIVER1.shutdowns.load(Ordering::Relaxed), shutdowns.0 + 1);
        assert_eq!(TRANSCEIVER2.shutdowns.load(Ordering::Relaxed), shutdowns.1 + 1);
        assert_eq!(CONTAINER_SHUTDOWNS.load(Ordering::Relaxed), done + 1);
    }
}
