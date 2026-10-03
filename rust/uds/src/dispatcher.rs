// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The transport layer that turns requests into jobs: the port of `DiagDispatcher.h`
//! (`DiagDispatcher2`, as the demo is built with) and `connection/DiagConnectionManager.h`,
//! without outgoing connections.

use core::cell::Cell;
use core::marker::PhantomData;

use openbsw_async::{QueueNode, Runnable};
use openbsw_timer::Lock;
use openbsw_transport::{
    DefaultTransportMessageProcessedListener, LayerErrorCode, LayerNode, ProcessingResult,
    ProvidingListenerHelper, ReceiveResult, ShutdownDelegate, TransportLayer, TransportMessage,
    TransportMessageListener, TransportMessageProcessedListener, TransportMessageProvider,
};
use openbsw_util::logger::LoggerComponent;
use openbsw_util::{log_critical, log_debug, log_error, log_warn};

use crate::UDS;
use crate::codes::DiagReturnCode;
use crate::config::{DiagnosisConfig, TransportJob};
use crate::connection::{DiagConnectionOwner, IncomingDiagConnection};
use crate::job::{DiagJob, DiagJobRoot, JobErrorCode};
use crate::session::DiagSessionManager;
use crate::uds_config::BUSY_MESSAGE_EXTRA_BYTES;

/// What a session control needs of the dispatcher: to switch it off for a programming
/// session (`IDiagDispatcher::disable`).
pub trait DispatcherControl: Sync {
    /// Refuse new requests.
    fn disable(&self);
    /// Take requests again.
    fn enable(&self);
    /// Whether requests are taken.
    fn is_enabled(&self) -> bool;
}

/// Hands out connections and takes them back: the port of `DiagConnectionManager`.
pub struct DiagConnectionManager<L: Lock + 'static> {
    config: &'static dyn DiagnosisConfig,
    outgoing_sender: &'static dyn TransportLayer,
    outgoing_provider: &'static dyn TransportMessageProvider,
    dispatcher: &'static DiagDispatcher<L>,
    shutdown_requested: Cell<bool>,
    _lock: PhantomData<fn() -> L>,
}

// SAFETY: the flag changes on the diagnostic context.
unsafe impl<L: Lock + 'static> Sync for DiagConnectionManager<L> {}

impl<L: Lock + 'static> DiagConnectionManager<L> {
    /// A manager of `config`'s connections, answering through `outgoing_sender` with
    /// messages from `outgoing_provider`, for `dispatcher`.
    pub const fn new(
        config: &'static dyn DiagnosisConfig,
        outgoing_sender: &'static dyn TransportLayer,
        outgoing_provider: &'static dyn TransportMessageProvider,
        dispatcher: &'static DiagDispatcher<L>,
    ) -> Self {
        Self {
            config,
            outgoing_sender,
            outgoing_provider,
            dispatcher,
            shutdown_requested: Cell::new(false),
            _lock: PhantomData,
        }
    }

    /// Take requests again.
    pub fn init(&self) {
        self.shutdown_requested.set(false);
    }

    /// Stop taking requests and report once every connection is back.
    pub fn shutdown(&'static self) {
        self.shutdown_requested.set(true);
        self.check_shutdown_progress();
    }

    /// Whether response-pending answers are sent.
    pub fn is_pending_activated(&self) -> bool {
        self.config.activate_outgoing_pending()
    }

    /// The bus of the layer.
    pub fn bus_id(&self) -> u8 {
        self.config.diag_bus_id()
    }

    /// Open a connection for `request`, if one is free and no shutdown is under way.
    pub fn request_incoming_connection(
        &'static self,
        request: &'static TransportMessage,
    ) -> Option<&'static IncomingDiagConnection> {
        if self.shutdown_requested.get() {
            return None;
        }
        let connection = {
            let _lock = L::lock();
            self.config.acquire_incoming_diag_connection()
        };
        let Some(connection) = connection else {
            log_warn!(
                UDS,
                b"No incoming diag connection available for 0x%x --> 0x%x, service 0x%x",
                request.source_id(),
                request.target_id(),
                request.service_id()
            );
            return None;
        };
        connection.assign(
            self,
            self.outgoing_sender,
            self.dispatcher.session_manager(),
            request,
            self.config.transport().functional_address,
        );
        connection.open(self.config.activate_outgoing_pending());
        Some(connection)
    }

    /// Nothing waits for an outgoing response here.
    pub fn process_pending_responses(&self) {}

    fn check_shutdown_progress(&'static self) {
        if !self.shutdown_requested.get() {
            return;
        }
        let in_use = self.config.incoming_connections_in_use();
        let total = self.config.incoming_connection_count();
        let outgoing = self.config.outgoing_connection_count();
        // The C++ asks the pool whether it is `full()`: a pool with free slots reports a
        // problem, so a quiet shutdown logs this line.
        if in_use != total {
            log_error!(
                UDS,
                b"DiagConnectionManager::problem at shutdown(in: %d/%d, out: %d/%d)",
                in_use as u32,
                total as u32,
                outgoing as u32,
                outgoing as u32
            );
            self.config.clear_incoming_diag_connections();
        }
        log_debug!(UDS, b"DiagConnectionManager shutdown complete");
        self.dispatcher.connection_manager_shutdown_complete();
    }
}

impl<L: Lock + 'static> DiagConnectionOwner for DiagConnectionManager<L> {
    fn diag_connection_terminated(&self, connection: &'static IncomingDiagConnection) {
        if let (Some(listener), Some(request)) =
            (connection.request_notification_listener(), connection.request_message())
        {
            request.reset_valid_bytes();
            let _ = request.increase_valid_bytes(request.payload_length());
            listener.transport_message_processed(request, ProcessingResult::NoError);
        }
        if let Some(response) = connection.response_message() {
            self.outgoing_provider.release_transport_message(response);
        }
        {
            let _lock = L::lock();
            self.config.release_incoming_diag_connection(connection);
        }
        // SAFETY: the manager is a field of the dispatcher, a `static` of the UDS system.
        let this: &'static Self = unsafe { &*(self as *const Self) };
        this.check_shutdown_progress();
    }

    fn source_diag_id(&self) -> u16 {
        self.config.diag_address()
    }
}

/// The runnable that drains the send queue on the diagnostic context.
struct ProcessQueue<L: Lock + 'static> {
    owner: &'static DiagDispatcher<L>,
    node: QueueNode<dyn Runnable>,
}

impl<L: Lock + 'static> Runnable for ProcessQueue<L> {
    fn execute(&self) {
        self.owner.process_queue();
    }

    fn node(&self) -> &QueueNode<dyn Runnable> {
        &self.node
    }
}

const BUSY_MESSAGE_LENGTH: usize = 3;

/// The diagnostic transport layer: the port of `DiagDispatcher2`. The router hands it the
/// requests for this ECU; it opens a connection for each and runs it through the job tree.
pub struct DiagDispatcher<L: Lock + 'static> {
    config: &'static dyn DiagnosisConfig,
    helper: ProvidingListenerHelper,
    node: LayerNode,
    session_manager: &'static dyn DiagSessionManager,
    job_root: &'static DiagJobRoot,
    enabled: Cell<bool>,
    connection_manager: DiagConnectionManager<L>,
    shutdown_delegate: Cell<Option<ShutdownDelegate>>,
    default_processed_listener: DefaultTransportMessageProcessedListener,
    busy_message: TransportMessage,
    busy_message_buffer: [Cell<u8>; BUSY_MESSAGE_LENGTH + BUSY_MESSAGE_EXTRA_BYTES],
    process_queue_call: ProcessQueue<L>,
    global: &'static LoggerComponent,
}

// SAFETY: the flags change on the diagnostic context or during lifecycle transitions; the
// queue under `L`.
unsafe impl<L: Lock + 'static> Sync for DiagDispatcher<L> {}

impl<L: Lock + 'static> DiagDispatcher<L> {
    /// A dispatcher over `config`, with `session_manager` and the jobs below `job_root`,
    /// given its own `'static` address; `bus_id` is `config`'s diagnostic bus again, since
    /// a `const fn` cannot ask the configuration; `global` is the application's GLOBAL log
    /// component, which the busy paths shout at.
    pub const fn new(
        this: &'static Self,
        config: &'static dyn DiagnosisConfig,
        bus_id: u8,
        session_manager: &'static dyn DiagSessionManager,
        job_root: &'static DiagJobRoot,
        global: &'static LoggerComponent,
    ) -> Self {
        Self {
            config,
            helper: ProvidingListenerHelper::new(bus_id),
            node: LayerNode::new(),
            session_manager,
            job_root,
            enabled: Cell::new(true),
            connection_manager: DiagConnectionManager::new(config, this, &this.helper, this),
            shutdown_delegate: Cell::new(None),
            default_processed_listener: DefaultTransportMessageProcessedListener,
            busy_message: TransportMessage::with_buffer(&this.busy_message_buffer),
            busy_message_buffer: [const { Cell::new(0) };
                BUSY_MESSAGE_LENGTH + BUSY_MESSAGE_EXTRA_BYTES],
            process_queue_call: ProcessQueue { owner: this, node: QueueNode::new() },
            global,
        }
    }

    /// The session manager (`IDiagDispatcher::getDiagSessionManager`).
    pub fn session_manager(&self) -> &'static dyn DiagSessionManager {
        self.session_manager
    }

    /// The connection manager.
    pub fn connection_manager(&self) -> &DiagConnectionManager<L> {
        &self.connection_manager
    }

    /// Add `job` to the tree (`IDiagDispatcher::addAbstractDiagJob`).
    pub fn add_abstract_diag_job(&self, job: &'static dyn DiagJob) -> JobErrorCode {
        self.job_root.base().add_abstract_diag_job(job)
    }

    /// Take `job` out of the tree (`IDiagDispatcher::removeAbstractDiagJob`).
    pub fn remove_abstract_diag_job(&self, job: &'static dyn DiagJob) {
        self.job_root.base().remove_abstract_diag_job(job);
    }

    /// This ECU's address (`getSourceId`).
    pub fn source_id(&self) -> u16 {
        self.config.diag_address()
    }

    /// Queue `message` as a request resuming after a reset (`resume`).
    pub fn resume(
        &'static self,
        message: &'static TransportMessage,
        listener: Option<&'static dyn TransportMessageProcessedListener>,
    ) -> LayerErrorCode {
        let target = message.target_id();
        let broadcast = self.config.broadcast_address();
        if target != self.config.diag_address()
            && (broadcast == TransportMessage::INVALID_ADDRESS || target != broadcast)
        {
            log_error!(
                UDS,
                b"DiagDispatcher::resume(): invalid target 0x%x, expected 0x%x",
                target,
                self.config.diag_address()
            );
            return LayerErrorCode::SendFail;
        }
        message.set_target_address(TransportMessage::INVALID_ADDRESS);
        self.enqueue_message(message, listener)
    }

    fn send_local(
        &'static self,
        message: &'static TransportMessage,
        listener: Option<&'static dyn TransportMessageProcessedListener>,
    ) -> LayerErrorCode {
        // Is this a response sent by one of our incoming connections?
        if let Some(listener) = listener
            && self.config.find_incoming_diag_connection(listener).is_some()
        {
            let status =
                self.helper.message_received(self.config.diag_bus_id(), message, Some(listener));
            return if status == ReceiveResult::NoError {
                LayerErrorCode::Ok
            } else {
                LayerErrorCode::SendFail
            };
        }
        let target = message.target_id();
        let broadcast = self.config.broadcast_address();
        if target != self.config.diag_address()
            && (broadcast != TransportMessage::INVALID_ADDRESS && target != broadcast)
        {
            log_error!(
                UDS,
                b"DiagDispatcher::send(): invalid target 0x%x, expected 0x%x",
                target,
                self.config.diag_address()
            );
            return LayerErrorCode::SendFail;
        }
        self.enqueue_message(message, listener)
    }

    fn enqueue_message(
        &'static self,
        message: &'static TransportMessage,
        listener: Option<&'static dyn TransportMessageProcessedListener>,
    ) -> LayerErrorCode {
        if !self.enabled.get() {
            return LayerErrorCode::SendFail;
        }
        if !message.is_complete() {
            return LayerErrorCode::MessageIncomplete;
        }
        let pushed = {
            let _lock = L::lock();
            self.config.push_send_job(TransportJob {
                message,
                processed_listener: listener.unwrap_or(&self.default_processed_listener),
            })
        };
        if pushed {
            self.trigger();
            return LayerErrorCode::Ok;
        }
        log_warn!(UDS, b"SendJobQueue full.");
        LayerErrorCode::QueueFull
    }

    fn process_queue(&'static self) {
        loop {
            let job = {
                let _lock = L::lock();
                self.config.pop_send_job()
            };
            let Some(job) = job else {
                break;
            };
            self.dispatch_incoming_request(job);
        }
        self.connection_manager.process_pending_responses();
    }

    fn dispatch_incoming_request(&'static self, mut job: TransportJob) {
        let is_resuming = job.message.target_id() == TransportMessage::INVALID_ADDRESS;
        if is_resuming {
            job.message.set_target_address(self.config.diag_address());
        }
        if !self.config.accept_all_requests()
            && !self.config.transport().is_from_tester(job.message)
        {
            // Check if the source is a tester or a functional request.
            log_warn!(UDS, b"Request from invalid source 0x%x discarded", job.message.source_id());
            job.processed_listener
                .transport_message_processed(job.message, ProcessingResult::ErrorGeneral);
            return;
        }
        let mut send_busy_negative_response = false;
        if self.config.copy_functional_requests()
            && self.config.transport().is_functionally_addressed(job.message)
        {
            match self.copy_functional_request(job.message) {
                Some(copy) => {
                    job.processed_listener
                        .transport_message_processed(job.message, ProcessingResult::NoError);
                    job.processed_listener = self;
                    job.message = copy;
                }
                None => {
                    log_critical!(self.global, b"!!!! Busy because no functional buffer!");
                    send_busy_negative_response = true;
                }
            }
        }
        if !send_busy_negative_response {
            match self.connection_manager.request_incoming_connection(job.message) {
                Some(connection) => {
                    log_debug!(
                        UDS,
                        b"Opening incoming connection 0x%x --> 0x%x, service 0x%x",
                        connection.source_address(),
                        connection.target_address(),
                        connection.service_id()
                    );
                    connection.set_resuming(is_resuming);
                    connection.set_request_notification_listener(job.processed_listener);
                    let result = self.job_root.execute(connection, job.message.payload());
                    if result != DiagReturnCode::Ok {
                        let _ = connection.send_negative_response(result.as_byte(), self.job_root);
                        connection.terminate();
                    }
                }
                None => {
                    log_critical!(self.global, b"!!!! Busy because no incoming connection!");
                    send_busy_negative_response = true;
                }
            }
        }
        if send_busy_negative_response {
            self.send_busy_response(job.message);
            job.processed_listener
                .transport_message_processed(job.message, ProcessingResult::NoError);
        }
    }

    fn copy_functional_request(
        &self,
        request: &TransportMessage,
    ) -> Option<&'static TransportMessage> {
        let copy = self
            .helper
            .get_transport_message(
                self.config.diag_bus_id(),
                request.source_id(),
                self.config.diag_address(),
                self.config.transport().diag_payload_size,
                &[],
            )
            .ok()?;
        copy.reset_valid_bytes();
        copy.set_source_address(request.source_id());
        copy.set_target_address(request.target_id());
        copy.set_payload_length(request.payload_length());
        for byte in request.payload() {
            let _ = copy.append_byte(byte.get());
        }
        Some(copy)
    }

    fn send_busy_response(&'static self, message: &TransportMessage) {
        log_error!(UDS, b"No incoming connection available -> request discarded --> BUSY");
        self.busy_message.set_source_address(self.config.diag_address());
        self.busy_message.set_target_address(message.source_id());
        self.busy_message_buffer[1].set(message.service_id());
        let status =
            self.helper.message_received(self.config.diag_bus_id(), &self.busy_message, None);
        if status != ReceiveResult::NoError {
            log_error!(UDS, b"Could not send BUSY_REPEAT_REQUEST!");
        }
    }

    fn trigger(&'static self) {
        openbsw_async::execute(self.config.context(), &self.process_queue_call);
    }

    fn connection_manager_shutdown_complete(&'static self) {
        log_debug!(UDS, b"DiagDispatcher2::connectionManagerShutdownComplete()");
        if let Some(delegate) = self.shutdown_delegate.get() {
            delegate.shutdown_done(self);
        }
    }
}

impl<L: Lock + 'static> DispatcherControl for DiagDispatcher<L> {
    fn disable(&self) {
        self.enabled.set(false);
    }

    fn enable(&self) {
        self.enabled.set(true);
    }

    fn is_enabled(&self) -> bool {
        self.enabled.get()
    }
}

impl<L: Lock + 'static> TransportLayer for DiagDispatcher<L> {
    fn bus_id(&self) -> u8 {
        self.config.diag_bus_id()
    }

    fn init(&'static self) -> LayerErrorCode {
        self.busy_message.reset_valid_bytes();
        let _ = self.busy_message.append_byte(DiagReturnCode::NEGATIVE_RESPONSE_IDENTIFIER);
        let _ = self.busy_message.append_byte(0x00);
        let _ = self.busy_message.append_byte(DiagReturnCode::IsoBusyRepeatRequest.as_byte());
        self.busy_message.set_payload_length(BUSY_MESSAGE_LENGTH as u16);
        self.connection_manager.init();
        self.enable();
        LayerErrorCode::Ok
    }

    fn shutdown(&'static self, done: ShutdownDelegate) -> bool {
        log_debug!(UDS, b"DiagDispatcher2::shutdown()");
        self.disable();
        self.shutdown_delegate.set(Some(done));
        self.connection_manager.shutdown();
        false
    }

    fn send(
        &'static self,
        message: &'static TransportMessage,
        notification: Option<&'static dyn TransportMessageProcessedListener>,
    ) -> LayerErrorCode {
        self.send_local(message, notification)
    }

    fn providing_listener_helper(&self) -> &ProvidingListenerHelper {
        &self.helper
    }

    fn node(&self) -> &LayerNode {
        &self.node
    }
}

impl<L: Lock + 'static> TransportMessageProcessedListener for DiagDispatcher<L> {
    fn transport_message_processed(
        &self,
        message: &'static TransportMessage,
        _result: ProcessingResult,
    ) {
        self.helper.release_transport_message(message);
    }
}

// The demo's UDS system end to end, against the responses the C++ demo gives in Forkpoint:
// the dispatcher, the session control, the services the demo registers, and a router that
// answers like TransportRouterSimple with a DoCAN layer behind it.
#[cfg(test)]
mod tests {
    extern crate std;

    use core::cell::Cell;
    use std::boxed::Box;
    use std::sync::Mutex;
    use std::vec::Vec;

    use openbsw_async::mock::MockAsync;
    use openbsw_timer::NoLock;
    use openbsw_transport::{
        ProviderErrorCode, ReceiveResult, ShutdownListener, TransportMessageListener,
    };

    use super::*;
    use crate::config::{DiagnosisConfiguration, TransportConfiguration};
    use crate::job::tests::{cells, serial};
    use crate::job::{Request, set_default_diag_session_manager, set_diag_job_root};
    use crate::jobs::{DataIdentifierJob, ReadIdentifierFromMemory};
    use crate::lifecycle::{ShutdownType, UdsLifecycleConnector};
    use crate::persistence::SessionPersistence;
    use crate::services::{
        CommunicationControl, DiagnosticSessionControl, ReadDataByIdentifier,
        RequestRoutineResults, RoutineControl, StartRoutine, StopRoutine, TesterPresent,
        WriteDataByIdentifier,
    };
    use crate::session::{
        APPLICATION_DEFAULT_SESSION, APPLICATION_EXTENDED_SESSION, DiagSessionMask, SessionType,
    };

    const CONTEXT: openbsw_async::ContextType = 1;
    const SELFDIAG: u8 = 1;
    const LOGICAL_ADDRESS: u16 = 0x2A;
    const TESTER: u16 = 0xF0;

    static MOCK_ASYNC: MockAsync<2> = MockAsync::new([b"main" as &[u8], b"uds"]);

    /// The transport router as the dispatcher sees it: hands out messages and takes the
    /// answers, which it "sends" at once, as a DoCAN layer would in time.
    struct MockRouter {
        messages: [&'static TransportMessage; 3],
        in_use: [Cell<bool>; 3],
        responses: Mutex<Vec<(u16, u16, Vec<u8>)>>,
    }

    // SAFETY: a test fixture.
    unsafe impl Sync for MockRouter {}

    impl MockRouter {
        fn take_responses(&self) -> Vec<(u16, u16, Vec<u8>)> {
            core::mem::take(
                &mut *self.responses.lock().unwrap_or_else(|poisoned| poisoned.into_inner()),
            )
        }
    }

    impl TransportMessageProvider for MockRouter {
        fn get_transport_message(
            &self,
            _src_bus_id: u8,
            _source_address: u16,
            _target_address: u16,
            _size: u16,
            _peek: &[u8],
        ) -> Result<&'static TransportMessage, ProviderErrorCode> {
            for (message, in_use) in self.messages.iter().zip(&self.in_use) {
                if !in_use.get() {
                    in_use.set(true);
                    return Ok(message);
                }
            }
            Err(ProviderErrorCode::NoMsgAvailable)
        }

        fn release_transport_message(&self, message: &'static TransportMessage) {
            for (pooled, in_use) in self.messages.iter().zip(&self.in_use) {
                if core::ptr::eq(*pooled, message) {
                    in_use.set(false);
                }
            }
        }
    }

    impl TransportMessageListener for MockRouter {
        fn message_received(
            &self,
            _source_bus_id: u8,
            message: &'static TransportMessage,
            notification: Option<&'static dyn TransportMessageProcessedListener>,
        ) -> ReceiveResult {
            self.responses.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push((
                message.source_id(),
                message.target_id(),
                message.payload().iter().map(Cell::get).collect(),
            ));
            if let Some(notification) = notification {
                notification.transport_message_processed(message, ProcessingResult::NoError);
            }
            ReceiveResult::NoError
        }
    }

    fn leak_message() -> &'static TransportMessage {
        Box::leak(Box::new(TransportMessage::with_buffer(cells(&std::vec![0; 4095]))))
    }

    static ROUTER: std::sync::LazyLock<MockRouter> = std::sync::LazyLock::new(|| MockRouter {
        messages: [leak_message(), leak_message(), leak_message()],
        in_use: [const { Cell::new(false) }; 3],
        responses: Mutex::new(Vec::new()),
    });

    /// The DoCAN layer's side of a request: gives the message back to the router.
    struct RequestListener(Mutex<Vec<ProcessingResult>>);

    impl TransportMessageProcessedListener for RequestListener {
        fn transport_message_processed(
            &self,
            message: &'static TransportMessage,
            result: ProcessingResult,
        ) {
            self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(result);
            ROUTER.release_transport_message(message);
        }
    }

    static REQUEST_LISTENER: RequestListener = RequestListener(Mutex::new(Vec::new()));

    struct Lifecycle;

    impl UdsLifecycleConnector for Lifecycle {
        fn is_mode_change_possible(&self) -> bool {
            true
        }

        fn request_powerdown(&self, _rapid: bool, _time: &mut u8) -> bool {
            true
        }

        fn request_shutdown(&self, _shutdown_type: ShutdownType, _timeout: u32) -> bool {
            true
        }
    }

    struct DummyPersistence;

    impl SessionPersistence for DummyPersistence {
        fn read_session(&self, _session_control: &'static DiagnosticSessionControl) {}

        fn write_session(&self, _session_control: &'static DiagnosticSessionControl, _session: u8) {
        }
    }

    /// The demo's `ReadIdentifierPot`: 0xF102 answers a fixed reading.
    struct ReadIdentifierPot {
        job: DataIdentifierJob,
    }

    impl DiagJob for ReadIdentifierPot {
        fn base(&self) -> &crate::job::JobBase {
            self.job.base()
        }

        fn verify(&self, request: Request<'_>) -> DiagReturnCode {
            self.job.verify(request)
        }

        fn process(
            &'static self,
            connection: &'static IncomingDiagConnection,
            _request: Request<'_>,
        ) -> DiagReturnCode {
            let response = connection.release_request_get_response();
            let _ = response.append_data(&0x1234_5678u32.to_be_bytes());
            let _ = connection.send_positive_response_internal(response.length() as u16, self);
            DiagReturnCode::Ok
        }
    }

    static RESPONSE_DATA_22F186: [u8; 24] = [
        0x01, 0x02, 0x00, 0x02, 0x22, 0x02, 0x16, 0x0F, 0x01, 0x00, 0x00, 0x6D, 0x2F, 0x00, 0x00,
        0x01, 0x06, 0x00, 0x00, 0x8F, 0xE0, 0x00, 0x00, 0x01,
    ];
    static LIFECYCLE: Lifecycle = Lifecycle;
    static PERSISTENCE: DummyPersistence = DummyPersistence;
    static GLOBAL: LoggerComponent = LoggerComponent::new();
    static JOB_ROOT: DiagJobRoot = DiagJobRoot::new();
    static SESSION_CONTROL: DiagnosticSessionControl =
        DiagnosticSessionControl::new(&LIFECYCLE, CONTEXT, &PERSISTENCE);
    static COMMUNICATION_CONTROL: CommunicationControl = CommunicationControl::with_default_mask();
    static CONFIGURATION: DiagnosisConfiguration<5, 1, 16> = DiagnosisConfiguration::new(
        LOGICAL_ADDRESS,
        0x00DF,
        SELFDIAG,
        4095,
        true,
        false,
        true,
        CONTEXT,
        TransportConfiguration::DEMO,
    );
    static DISPATCHER: DiagDispatcher<NoLock> = DiagDispatcher::new(
        &DISPATCHER,
        &CONFIGURATION,
        SELFDIAG,
        &SESSION_CONTROL,
        &JOB_ROOT,
        &GLOBAL,
    );
    static READ_DATA_BY_IDENTIFIER: ReadDataByIdentifier = ReadDataByIdentifier::new();
    static WRITE_DATA_BY_IDENTIFIER: WriteDataByIdentifier = WriteDataByIdentifier::new();
    static ROUTINE_CONTROL: RoutineControl = RoutineControl::new();
    static START_ROUTINE: StartRoutine = StartRoutine::new();
    static STOP_ROUTINE: StopRoutine = StopRoutine::new();
    static REQUEST_ROUTINE_RESULTS: RequestRoutineResults = RequestRoutineResults::new();
    static READ_22F186: ReadIdentifierFromMemory =
        ReadIdentifierFromMemory::new(0xF186, &RESPONSE_DATA_22F186, DiagSessionMask::ALL_SESSIONS);
    static READ_22F102: ReadIdentifierPot = ReadIdentifierPot {
        job: DataIdentifierJob::new(&[0x22, 0xF1, 0x02], DiagSessionMask::ALL_SESSIONS),
    };
    static TESTER_PRESENT: TesterPresent = TesterPresent::new();

    struct ShutdownDone(Cell<u32>);

    // SAFETY: a test fixture.
    unsafe impl Sync for ShutdownDone {}

    impl ShutdownListener for ShutdownDone {
        fn shutdown_done(&self, layer: &'static dyn TransportLayer) {
            assert_eq!(layer.bus_id(), SELFDIAG);
            self.0.set(self.0.get() + 1);
        }
    }

    static SHUTDOWN_DONE: ShutdownDone = ShutdownDone(Cell::new(0));

    /// `UdsSystem::init` and the router's `addTransportLayer`.
    fn init() {
        openbsw_async::set_binding(&MOCK_ASYNC);
        assert_eq!(DISPATCHER.init(), LayerErrorCode::Ok);
        set_default_diag_session_manager(Some(&SESSION_CONTROL));
        set_diag_job_root(Some(&JOB_ROOT));
        SESSION_CONTROL.set_diag_dispatcher(Some(&DISPATCHER));
        DISPATCHER.providing_listener_helper().set_provider(Some(&*ROUTER));
        DISPATCHER.providing_listener_helper().set_listener(Some(&*ROUTER));
        for job in [
            &READ_DATA_BY_IDENTIFIER as &'static dyn DiagJob,
            &READ_22F186,
            &READ_22F102,
            &WRITE_DATA_BY_IDENTIFIER,
            &ROUTINE_CONTROL,
            &START_ROUTINE,
            &STOP_ROUTINE,
            &REQUEST_ROUTINE_RESULTS,
            &TESTER_PRESENT,
            &SESSION_CONTROL,
            &COMMUNICATION_CONTROL,
        ] {
            let _ = DISPATCHER.add_abstract_diag_job(job);
        }
        ROUTER.take_responses();
        REQUEST_LISTENER.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clear();
    }

    /// A tester's request arrives through the router, is dispatched, and answered.
    fn request(payload: &[u8]) -> Vec<(u16, u16, Vec<u8>)> {
        let message =
            ROUTER.get_transport_message(SELFDIAG, TESTER, LOGICAL_ADDRESS, 4095, &[]).unwrap();
        message.reset_valid_bytes();
        message.set_source_address(TESTER);
        message.set_target_address(LOGICAL_ADDRESS);
        message.set_payload_length(payload.len() as u16);
        let _ = message.append(payload);
        assert_eq!(DISPATCHER.send(message, Some(&REQUEST_LISTENER)), LayerErrorCode::Ok);
        MOCK_ASYNC.run_runnables(CONTEXT);
        ROUTER.take_responses()
    }

    fn answer(payload: &[u8]) -> Vec<u8> {
        let responses = request(payload);
        assert_eq!(responses.len(), 1, "one answer to {payload:02x?}, got {responses:02x?}");
        assert_eq!((responses[0].0, responses[0].1), (LOGICAL_ADDRESS, TESTER));
        responses[0].2.clone()
    }

    #[test]
    fn the_demo_exchange() {
        let _guard = serial();
        init();
        // TesterPresent, then suppressed.
        assert_eq!(answer(&[0x3E, 0x00]), [0x7E, 0x00]);
        assert!(request(&[0x3E, 0x80]).is_empty());
        // ReadDataByIdentifier 0xF186 and 0xF102.
        let mut expected = std::vec![0x62, 0xF1, 0x86];
        expected.extend_from_slice(&RESPONSE_DATA_22F186);
        assert_eq!(answer(&[0x22, 0xF1, 0x86]), expected);
        assert_eq!(answer(&[0x22, 0xF1, 0x02]), [0x62, 0xF1, 0x02, 0x12, 0x34, 0x56, 0x78]);
        // Into the extended session and back.
        assert_eq!(answer(&[0x10, 0x03]), [0x50, 0x03, 0x00, 0x32, 0x01, 0xF4]);
        assert!(core::ptr::eq(SESSION_CONTROL.active_session(), &APPLICATION_EXTENDED_SESSION));
        assert_eq!(answer(&[0x3E, 0x00]), [0x7E, 0x00]);
        assert_eq!(answer(&[0x10, 0x01]), [0x50, 0x01, 0x00, 0x32, 0x01, 0xF4]);
        assert!(core::ptr::eq(SESSION_CONTROL.active_session(), &APPLICATION_DEFAULT_SESSION));
        // Negative responses: no ECUReset, an unknown identifier, programming from default,
        // CommunicationControl outside the extended session.
        assert_eq!(answer(&[0x11, 0x01]), [0x7F, 0x11, 0x11]);
        assert_eq!(answer(&[0x22, 0xF1, 0x90]), [0x7F, 0x22, 0x31]);
        assert_eq!(answer(&[0x10, 0x02]), [0x7F, 0x10, 0x7E]);
        assert_eq!(answer(&[0x28, 0x00]), [0x7F, 0x28, 0x7F]);
        // Every request message went back to the router; every connection is free.
        assert!(ROUTER.in_use.iter().all(|in_use| !in_use.get()));
        assert_eq!(CONFIGURATION.incoming_connections_in_use(), 0);
        assert_eq!(
            REQUEST_LISTENER.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).len(),
            11
        );
        // The extended session times out after five seconds without a TesterPresent.
        assert_eq!(answer(&[0x10, 0x03]), [0x50, 0x03, 0x00, 0x32, 0x01, 0xF4]);
        assert!(SESSION_CONTROL.is_session_timeout_active());
        MOCK_ASYNC.elapse(4_999_000);
        MOCK_ASYNC.run_timeouts(CONTEXT);
        assert!(core::ptr::eq(SESSION_CONTROL.active_session(), &APPLICATION_EXTENDED_SESSION));
        MOCK_ASYNC.elapse(2_000);
        MOCK_ASYNC.run_timeouts(CONTEXT);
        assert!(core::ptr::eq(SESSION_CONTROL.active_session(), &APPLICATION_DEFAULT_SESSION));
        assert_eq!(SESSION_CONTROL.active_session().session_type(), SessionType::Default);
        // Shutdown: the layer reports once the connections are counted.
        let done = SHUTDOWN_DONE.0.get();
        assert!(!DISPATCHER.shutdown(&SHUTDOWN_DONE));
        assert_eq!(SHUTDOWN_DONE.0.get(), done + 1);
        assert!(!DISPATCHER.is_enabled());
        // While shut down, requests are refused.
        let message =
            ROUTER.get_transport_message(SELFDIAG, TESTER, LOGICAL_ADDRESS, 4095, &[]).unwrap();
        message.set_payload_length(2);
        message.reset_valid_bytes();
        let _ = message.append(&[0x3E, 0x00]);
        assert_eq!(DISPATCHER.send(message, Some(&REQUEST_LISTENER)), LayerErrorCode::SendFail);
        ROUTER.release_transport_message(message);
    }

    #[test]
    fn requests_from_strangers_and_to_other_targets_are_refused() {
        let _guard = serial();
        init();
        let message =
            ROUTER.get_transport_message(SELFDIAG, 0x55, LOGICAL_ADDRESS, 4095, &[]).unwrap();
        message.reset_valid_bytes();
        message.set_source_address(0x55);
        message.set_target_address(LOGICAL_ADDRESS);
        message.set_payload_length(2);
        let _ = message.append(&[0x3E, 0x00]);
        assert_eq!(DISPATCHER.send(message, Some(&REQUEST_LISTENER)), LayerErrorCode::Ok);
        MOCK_ASYNC.run_runnables(CONTEXT);
        assert!(ROUTER.take_responses().is_empty());
        assert_eq!(
            REQUEST_LISTENER.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).last(),
            Some(&ProcessingResult::ErrorGeneral)
        );
        let message = ROUTER.get_transport_message(SELFDIAG, TESTER, 0x33, 4095, &[]).unwrap();
        message.reset_valid_bytes();
        message.set_source_address(TESTER);
        message.set_target_address(0x33);
        message.set_payload_length(2);
        let _ = message.append(&[0x3E, 0x00]);
        assert_eq!(DISPATCHER.send(message, Some(&REQUEST_LISTENER)), LayerErrorCode::SendFail);
        ROUTER.release_transport_message(message);
        assert!(ROUTER.in_use.iter().all(|in_use| !in_use.get()));
    }
}
