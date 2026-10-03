// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! One request from a tester and its answer: the port of
//! `connection/IncomingDiagConnection.h`, without nested requests.
//!
//! A connection is a slot of the dispatcher's pool. The manager opens it with the request
//! message, the job tree answers through it, and the transport layer's callbacks come back
//! on the diagnostic context through the connection's own runnables, as the C++ `Call`
//! closures do.

use core::cell::Cell;

use openbsw_async::{ContextType, QueueNode, Runnable, TimeUnit, Timeout};
use openbsw_transport::{
    LayerErrorCode, ProcessingResult, TransportLayer, TransportMessage,
    TransportMessageProcessedListener,
};
use openbsw_util::{log_critical, log_debug, log_error};

use crate::UDS;
use crate::codes::{ConnectionErrorCode, DiagCodes, DiagReturnCode};
use crate::job::{DiagJob, ResponseSendResult};
use crate::response::PositiveResponse;
use crate::session::DiagSessionManager;

/// Who hands out connections and hears when one closes: the manager's side of
/// `DiagConnectionManager`.
pub trait DiagConnectionOwner: Sync {
    /// `connection` is closed and free again.
    fn diag_connection_terminated(&self, connection: &'static IncomingDiagConnection);
    /// This ECU's address, the source of answers to functional requests.
    fn source_diag_id(&self) -> u16;
}

/// How the C++ `ErrorCode` enumerators number, for the log.
fn layer_error_number(code: LayerErrorCode) -> u32 {
    match code {
        LayerErrorCode::Ok => 0,
        LayerErrorCode::SendFail => 1,
        LayerErrorCode::QueueFull => 2,
        LayerErrorCode::MessageIncomplete => 3,
        LayerErrorCode::MessageAlreadyInProgress => 4,
        LayerErrorCode::GeneralError => 5,
    }
}

#[derive(Clone, Copy)]
enum Call {
    SendPositiveResponse,
    SendNegativeResponse,
    TransportMessageProcessed,
}

/// A deferred call on the connection's context (`::async::Call<closure>`).
struct ConnectionCall {
    owner: Cell<Option<&'static IncomingDiagConnection>>,
    call: Call,
    node: QueueNode<dyn Runnable>,
}

// SAFETY: the owner is set once the connection is bound, before any call is queued.
unsafe impl Sync for ConnectionCall {}

impl ConnectionCall {
    const fn new(call: Call) -> Self {
        Self { owner: Cell::new(None), call, node: QueueNode::new() }
    }
}

impl Runnable for ConnectionCall {
    fn execute(&self) {
        let owner = self.owner.get().expect("a queued call has its connection");
        match self.call {
            Call::SendPositiveResponse => owner.async_send_positive_response(),
            Call::SendNegativeResponse => owner.async_send_negative_response(),
            Call::TransportMessageProcessed => owner.async_transport_message_processed(),
        }
    }

    fn node(&self) -> &QueueNode<dyn Runnable> {
        &self.node
    }
}

/// One of the connection's two timers (`IncomingDiagConnection::Timeout`).
struct PendingTimeout {
    owner: Cell<Option<&'static IncomingDiagConnection>>,
    global: bool,
    timeout: Timeout,
    node: QueueNode<dyn Runnable>,
}

// SAFETY: as `ConnectionCall`.
unsafe impl Sync for PendingTimeout {}

impl PendingTimeout {
    const fn new(global: bool) -> Self {
        Self { owner: Cell::new(None), global, timeout: Timeout::new(), node: QueueNode::new() }
    }
}

impl Runnable for PendingTimeout {
    fn execute(&self) {
        let owner = self.owner.get().expect("a scheduled timeout has its connection");
        owner.expired(self.global);
    }

    fn node(&self) -> &QueueNode<dyn Runnable> {
        &self.node
    }
}

/// A request in progress: who sent it, the message it came in, the answer being built, and
/// the response-pending timers.
pub struct IncomingDiagConnection {
    in_use: Cell<bool>,
    context: Cell<ContextType>,
    open: Cell<bool>,
    source_id: Cell<u16>,
    target_id: Cell<u16>,
    service_id: Cell<u8>,
    functional_address: Cell<u16>,
    request_message: Cell<Option<&'static TransportMessage>>,
    response_message: Cell<Option<&'static TransportMessage>>,
    message_sender: Cell<Option<&'static dyn TransportLayer>>,
    connection_manager: Cell<Option<&'static dyn DiagConnectionOwner>>,
    diag_session_manager: Cell<Option<&'static dyn DiagSessionManager>>,
    request_notification_listener: Cell<Option<&'static dyn TransportMessageProcessedListener>>,
    sender: Cell<Option<&'static dyn DiagJob>>,
    response_pending_timeout: PendingTimeout,
    global_pending_timeout: PendingTimeout,
    send_positive_response_call: ConnectionCall,
    send_negative_response_call: ConnectionCall,
    transport_message_processed_call: ConnectionCall,
    positive_response_args: Cell<(u16, Option<&'static dyn DiagJob>)>,
    negative_response_args: Cell<(u8, Option<&'static dyn DiagJob>)>,
    processed_args: Cell<(Option<&'static TransportMessage>, ProcessingResult)>,
    num_pending_message_processed_callbacks: Cell<u8>,
    connection_termination_is_pending: Cell<bool>,
    suppress_positive_response: Cell<bool>,
    pending_activated: Cell<bool>,
    response_pending_is_pending: Cell<bool>,
    response_pending_sent: Cell<bool>,
    response_pending_is_being_sent: Cell<bool>,
    is_response_active: Cell<bool>,
    is_resuming: Cell<bool>,
    pending_timeout_ms: Cell<u32>,
    pending_message: TransportMessage,
    pending_message_buffer: [Cell<u8>; Self::PENDING_MESSAGE_BUFFER_LENGTH],
    response_message_storage: TransportMessage,
    positive_response: PositiveResponse,
    negative_response_temp_buffer: [Cell<u8>; DiagCodes::NEGATIVE_RESPONSE_MESSAGE_LENGTH as usize],
    identifiers: [Cell<u8>; Self::MAXIMUM_NUMBER_OF_IDENTIFIERS],
    identifier_count: Cell<u8>,
}

// SAFETY: a connection's state is touched on the diagnostic context: the dispatcher opens
// it there, jobs answer there, and the transport layer's callbacks are re-queued onto it.
unsafe impl Sync for IncomingDiagConnection {}

impl IncomingDiagConnection {
    /// The first response-pending answer goes out this long after the request.
    pub const INITIAL_PENDING_TIMEOUT_MS: u32 = 40;
    /// Then every this long.
    pub const DEFAULT_PENDING_TIMEOUT_MS: u32 = 4500;
    /// A request is given up after this long.
    pub const GLOBAL_PENDING_TIMEOUT_MS: u32 = 190_000;
    /// How many request bytes a job can claim as identifiers echoed in the answer.
    pub const MAXIMUM_NUMBER_OF_IDENTIFIERS: usize = 6;
    /// A response-pending answer: `7F <service> 78`.
    pub const PENDING_MESSAGE_PAYLOAD_LENGTH: u16 = 3;
    /// Its buffer.
    pub const PENDING_MESSAGE_BUFFER_LENGTH: usize = 3;

    /// A free connection; its context is set when the pool is configured.
    pub const fn new() -> Self {
        Self {
            in_use: Cell::new(false),
            context: Cell::new(openbsw_async::CONTEXT_INVALID),
            open: Cell::new(false),
            source_id: Cell::new(0xFF),
            target_id: Cell::new(0xFF),
            service_id: Cell::new(0xFF),
            functional_address: Cell::new(0xDF),
            request_message: Cell::new(None),
            response_message: Cell::new(None),
            message_sender: Cell::new(None),
            connection_manager: Cell::new(None),
            diag_session_manager: Cell::new(None),
            request_notification_listener: Cell::new(None),
            sender: Cell::new(None),
            response_pending_timeout: PendingTimeout::new(false),
            global_pending_timeout: PendingTimeout::new(true),
            send_positive_response_call: ConnectionCall::new(Call::SendPositiveResponse),
            send_negative_response_call: ConnectionCall::new(Call::SendNegativeResponse),
            transport_message_processed_call: ConnectionCall::new(Call::TransportMessageProcessed),
            positive_response_args: Cell::new((0, None)),
            negative_response_args: Cell::new((0, None)),
            processed_args: Cell::new((None, ProcessingResult::ErrorGeneral)),
            num_pending_message_processed_callbacks: Cell::new(0),
            connection_termination_is_pending: Cell::new(false),
            suppress_positive_response: Cell::new(false),
            pending_activated: Cell::new(true),
            response_pending_is_pending: Cell::new(false),
            response_pending_sent: Cell::new(false),
            response_pending_is_being_sent: Cell::new(false),
            is_response_active: Cell::new(false),
            is_resuming: Cell::new(false),
            pending_timeout_ms: Cell::new(Self::DEFAULT_PENDING_TIMEOUT_MS),
            pending_message: TransportMessage::new(),
            pending_message_buffer: [const { Cell::new(0) }; Self::PENDING_MESSAGE_BUFFER_LENGTH],
            response_message_storage: TransportMessage::new(),
            positive_response: PositiveResponse::new(),
            negative_response_temp_buffer: [const { Cell::new(0) };
                DiagCodes::NEGATIVE_RESPONSE_MESSAGE_LENGTH as usize],
            identifiers: [const { Cell::new(0) }; Self::MAXIMUM_NUMBER_OF_IDENTIFIERS],
            identifier_count: Cell::new(0),
        }
    }

    /// Give the connection its own `'static` address: its runnables and timers name it, and
    /// its pending message gets its buffer (the C++ constructor's work).
    pub fn bind(&'static self) {
        self.response_pending_timeout.owner.set(Some(self));
        self.global_pending_timeout.owner.set(Some(self));
        self.send_positive_response_call.owner.set(Some(self));
        self.send_negative_response_call.owner.set(Some(self));
        self.transport_message_processed_call.owner.set(Some(self));
        self.pending_message.init(&self.pending_message_buffer);
    }

    fn as_static(&self) -> &'static IncomingDiagConnection {
        self.transport_message_processed_call.owner.get().expect("the connection is bound")
    }

    pub(crate) fn is_in_use(&self) -> bool {
        self.in_use.get()
    }

    pub(crate) fn set_in_use(&self, in_use: bool) {
        self.in_use.set(in_use);
    }

    /// The context the connection runs on.
    pub fn context(&self) -> ContextType {
        self.context.get()
    }

    /// Set the context the connection runs on.
    pub fn set_context(&self, context: ContextType) {
        self.context.set(context);
    }

    /// The tester's address.
    pub fn source_address(&self) -> u16 {
        self.source_id.get()
    }

    /// The address the request was sent to.
    pub fn target_address(&self) -> u16 {
        self.target_id.get()
    }

    /// The request's service id.
    pub fn service_id(&self) -> u8 {
        self.service_id.get()
    }

    /// Whether the request went to the functional address.
    pub fn is_functionally_addressed(&self) -> bool {
        self.target_id.get() == self.functional_address.get()
    }

    /// The request message.
    pub fn request_message(&self) -> Option<&'static TransportMessage> {
        self.request_message.get()
    }

    /// The message the answer is built in, once one was taken.
    pub fn response_message(&self) -> Option<&'static TransportMessage> {
        self.response_message.get()
    }

    /// Who is told when the request message was processed.
    pub fn request_notification_listener(
        &self,
    ) -> Option<&'static dyn TransportMessageProcessedListener> {
        self.request_notification_listener.get()
    }

    /// Set who is told when the request message was processed.
    pub fn set_request_notification_listener(
        &self,
        listener: &'static dyn TransportMessageProcessedListener,
    ) {
        self.request_notification_listener.set(Some(listener));
    }

    /// Whether the request resumes one interrupted by a reset.
    pub fn is_resuming(&self) -> bool {
        self.is_resuming.get()
    }

    /// Mark the request as resuming one interrupted by a reset.
    pub fn set_resuming(&self, resuming: bool) {
        self.is_resuming.set(resuming);
    }

    /// Whether a response is being sent.
    pub fn is_busy(&self) -> bool {
        self.sender.get().is_some()
    }

    /// Set the session manager told about answers.
    pub fn set_diag_session_manager(&self, manager: &'static dyn DiagSessionManager) {
        self.diag_session_manager.set(Some(manager));
    }

    /// Fill in a request (what `DiagConnectionManager::requestIncomingConnection` sets).
    pub fn assign(
        &self,
        manager: &'static dyn DiagConnectionOwner,
        message_sender: &'static dyn TransportLayer,
        session_manager: &'static dyn DiagSessionManager,
        request: &'static TransportMessage,
        functional_address: u16,
    ) {
        self.connection_manager.set(Some(manager));
        self.message_sender.set(Some(message_sender));
        self.diag_session_manager.set(Some(session_manager));
        self.source_id.set(request.source_id());
        self.target_id.set(request.target_id());
        self.service_id.set(request.service_id());
        self.functional_address.set(functional_address);
        self.request_message.set(Some(request));
        self.response_message.set(None);
    }

    /// Set the addresses and service of a request by hand, as the tests do.
    pub fn set_addresses(&self, source: u16, target: u16, service: u8) {
        self.source_id.set(source);
        self.target_id.set(target);
        self.service_id.set(service);
    }

    /// Set the request message by hand, as the tests do.
    pub fn set_request_message(&self, request: Option<&'static TransportMessage>) {
        self.request_message.set(request);
    }

    /// Set the response message by hand, as the tests do.
    pub fn set_response_message(&self, response: Option<&'static TransportMessage>) {
        self.response_message.set(response);
    }

    /// Lengthen or shorten the time between response-pending answers.
    pub fn change_resp_pending_timer(&self, diff_time_ms: i32) {
        self.pending_timeout_ms
            .set((Self::DEFAULT_PENDING_TIMEOUT_MS as i32).wrapping_add(diff_time_ms) as u32);
    }

    /// Open for a request; with `activate_pending`, response-pending answers go out while
    /// the job takes its time.
    pub fn open(&'static self, activate_pending: bool) {
        self.bind();
        if self.open.get() {
            log_error!(UDS, b"IncomingDiagConnection::open(): opening already open connection!");
            return;
        }
        self.open.set(true);
        self.pending_activated.set(activate_pending);
        self.suppress_positive_response.set(false);
        self.response_pending_sent.set(false);
        self.response_pending_is_being_sent.set(false);
        self.is_response_active.set(false);
        self.identifier_count.set(0);
        self.response_pending_timeout.timeout.cancel();
        self.change_resp_pending_timer(0);
        if self.pending_activated.get() {
            openbsw_async::schedule(
                self.context.get(),
                &self.response_pending_timeout,
                &self.response_pending_timeout.timeout,
                Self::INITIAL_PENDING_TIMEOUT_MS,
                TimeUnit::Milliseconds,
            );
            openbsw_async::schedule(
                self.context.get(),
                &self.global_pending_timeout,
                &self.global_pending_timeout.timeout,
                Self::GLOBAL_PENDING_TIMEOUT_MS,
                TimeUnit::Milliseconds,
            );
        }
    }

    /// Whether the connection is open.
    pub fn is_open(&self) -> bool {
        self.open.get()
    }

    /// Close the connection once every response went out, and give it back.
    pub fn terminate(&'static self) {
        if !self.open.get() {
            return;
        }
        log_debug!(
            UDS,
            b"IncomingDiagConnection::terminate(): 0x%x --> 0x%x, service 0x%x",
            self.source_id.get(),
            self.target_id.get(),
            self.service_id.get()
        );
        if self.num_pending_message_processed_callbacks.get() != 0 {
            self.connection_termination_is_pending.set(true);
            return;
        }
        self.open.set(false);
        self.response_pending_timeout.timeout.cancel();
        self.global_pending_timeout.timeout.cancel();
        let Some(manager) = self.connection_manager.get() else {
            log_critical!(
                UDS,
                b"IncomingDiagConnection::terminate(): fpDiagConnectionManager == nullptr!"
            );
            panic!("diagnostic connection manager must not be null");
        };
        self.connection_termination_is_pending.set(false);
        self.sender.set(None);
        manager.diag_connection_terminated(self);
    }

    /// Claim the next request byte as an identifier the answer echoes.
    pub fn add_identifier(&self) {
        assert!(!self.is_response_active.get(), "response must not be active");
        let count = usize::from(self.identifier_count.get());
        if let Some(request) = self.request_message.get()
            && count < Self::MAXIMUM_NUMBER_OF_IDENTIFIERS
        {
            self.identifiers[count].set(request.buffer()[count].get());
            self.identifier_count.set(self.identifier_count.get() + 1);
        }
    }

    /// How many identifiers were claimed.
    pub fn num_identifiers(&self) -> u16 {
        u16::from(self.identifier_count.get())
    }

    /// The identifier at `idx`.
    pub fn identifier(&self, idx: u16) -> u8 {
        if idx >= self.num_identifiers() {
            log_error!(UDS, b"IncomingDiagConnection::addIdentifier(): invalid index %d!", idx);
            return 0;
        }
        self.identifiers[usize::from(idx)].get()
    }

    /// How many bytes an answer may carry after the identifiers.
    pub fn maximum_response_length(&self) -> u16 {
        self.request_message
            .get()
            .map_or(0, |request| request.max_payload_length() - self.num_identifiers())
    }

    /// Reuse the request's buffer for the answer and start the positive response in it,
    /// after the identifiers.
    pub fn release_request_get_response(&self) -> &PositiveResponse {
        if self.response_message.get().is_none() {
            let request = self.request_message.get().expect("a connection has a request");
            let this = self.as_static();
            this.response_message_storage.init(request.buffer());
            self.response_message.set(Some(&this.response_message_storage));
        }
        self.is_response_active.set(true);
        let response = self.response_message.get().expect("set above");
        let identifiers = usize::from(self.identifier_count.get());
        self.positive_response.init(&response.buffer()[identifiers.min(response.buffer().len())..]);
        &self.positive_response
    }

    /// Send what `sender` appended to the positive response.
    pub fn send_positive_response(
        &'static self,
        sender: &'static dyn DiagJob,
    ) -> ConnectionErrorCode {
        if !self.is_response_active.get() {
            let _ = self.release_request_get_response();
        }
        self.send_positive_response_internal(self.positive_response.length() as u16, sender)
    }

    /// Send `length` bytes of positive response for `sender`, on the context.
    pub fn send_positive_response_internal(
        &'static self,
        length: u16,
        sender: &'static dyn DiagJob,
    ) -> ConnectionErrorCode {
        if !self.open.get() {
            return ConnectionErrorCode::ConnectionNotOpen;
        }
        if self.message_sender.get().is_none() {
            return ConnectionErrorCode::SendFailed;
        }
        if self.sender.get().is_some() {
            log_error!(UDS, b"IncomingDiagConnection::sendPositiveResponse(): BUSY!");
            return ConnectionErrorCode::ConnectionBusy;
        }
        if self.response_message.get().is_none() {
            return ConnectionErrorCode::NoTpMessage;
        }
        self.num_pending_message_processed_callbacks
            .set(self.num_pending_message_processed_callbacks.get() + 1);
        self.positive_response_args.set((length, Some(sender)));
        openbsw_async::execute(self.context.get(), &self.send_positive_response_call);
        ConnectionErrorCode::Ok
    }

    fn async_send_positive_response(&'static self) {
        let (length, sender) = self.positive_response_args.get();
        let tmp_source_id = self.source_id.get();
        if let Some(response) = self.response_message.get() {
            self.set_source_id(response);
            response.set_target_address(tmp_source_id);
            if response.service_id() == DiagReturnCode::NEGATIVE_RESPONSE_IDENTIFIER {
                // We did send a negative response before: restore the payload.
                for (cell, saved) in
                    response.buffer().iter().zip(&self.negative_response_temp_buffer)
                {
                    cell.set(saved.get());
                }
            }
            response.reset_valid_bytes();
            let identifiers = usize::from(self.identifier_count.get());
            for identifier in &self.identifiers[..identifiers] {
                let _ = response.append_byte(identifier.get());
            }
            response.set_service_id(
                self.service_id.get().wrapping_add(DiagReturnCode::POSITIVE_RESPONSE_OFFSET),
            );
            let _ = response.increase_valid_bytes(length);
            response.set_payload_length(self.identifier_count.get() as u16 + length);
            self.sender.set(sender);
            let payload = &response.buffer()[identifiers..identifiers + usize::from(length)];
            if let Some(manager) = self.diag_session_manager.get() {
                manager.response_sent(self, DiagReturnCode::Ok, payload);
            }
        }
        if !self.suppress_positive_response.get() || self.response_pending_sent.get() {
            // This is not a positive response to a suppressed request: send.
            if !self.response_pending_is_being_sent.get() {
                let _ = self.send_response();
            }
        } else {
            // Ignore the response as it is suppressed.
            self.num_pending_message_processed_callbacks
                .set(self.num_pending_message_processed_callbacks.get() - 1);
            if let Some(sender) = self.sender.take() {
                sender.response_sent(self, ResponseSendResult::ResponseSent);
            }
        }
    }

    /// Send a negative response with `response_code` for `sender`, on the context.
    pub fn send_negative_response(
        &'static self,
        response_code: u8,
        sender: &'static dyn DiagJob,
    ) -> ConnectionErrorCode {
        if !self.open.get() {
            return ConnectionErrorCode::ConnectionNotOpen;
        }
        if self.message_sender.get().is_none() {
            log_error!(
                UDS,
                b"IncomingDiagConnection::sendNegativeResponse(): fpMessageSender is NULL!"
            );
            return ConnectionErrorCode::SendFailed;
        }
        if !self.is_response_active.get() {
            let _ = self.release_request_get_response();
        }
        if self.response_message.get().is_none() {
            return ConnectionErrorCode::NoTpMessage;
        }
        self.num_pending_message_processed_callbacks
            .set(self.num_pending_message_processed_callbacks.get() + 1);
        self.negative_response_args.set((response_code, Some(sender)));
        openbsw_async::execute(self.context.get(), &self.send_negative_response_call);
        ConnectionErrorCode::Ok
    }

    fn async_send_negative_response(&'static self) {
        let (response_code, sender) = self.negative_response_args.get();
        let response = self.response_message.get().expect("a negative response has a message");
        response.set_payload_length(u16::from(DiagCodes::NEGATIVE_RESPONSE_MESSAGE_LENGTH));
        self.set_source_id(response);
        response.set_target_address(self.source_id.get());
        if response.service_id() != DiagReturnCode::NEGATIVE_RESPONSE_IDENTIFIER {
            // Make a backup only the first time a negative response is sent!
            for (saved, cell) in self.negative_response_temp_buffer.iter().zip(response.buffer()) {
                saved.set(cell.get());
            }
        }
        response.reset_valid_bytes();
        let _ = response.append_byte(DiagReturnCode::NEGATIVE_RESPONSE_IDENTIFIER);
        let _ = response.append_byte(self.service_id.get());
        let _ = response.append_byte(response_code);
        self.sender.set(sender);
        if response_code != DiagReturnCode::IsoResponsePending.as_byte()
            && let Some(manager) = self.diag_session_manager.get()
        {
            let end = usize::from(DiagCodes::NEGATIVE_RESPONSE_MESSAGE_LENGTH);
            manager.response_sent(
                self,
                DiagReturnCode::from_byte(response_code),
                &response.buffer()[end..end],
            );
        }
        let suppressed_functional = self.is_functionally_addressed()
            && matches!(
                response_code,
                0x11 | 0x12 | 0x31 | 0x7E | 0x7F // SNS, SFNS, ROOR, SFNSIAS, SNSIAS
            );
        if !suppressed_functional {
            // This is no SNS or SFNS to a functional request: send the response.
            if !self.response_pending_is_being_sent.get() {
                if response_code == DiagReturnCode::IsoResponsePending.as_byte() {
                    self.restart_pending_timeout();
                }
                let _ = self.send_response();
            }
        } else {
            // Ignore the response as it is suppressed in this case.
            self.num_pending_message_processed_callbacks
                .set(self.num_pending_message_processed_callbacks.get() - 1);
            if let Some(sender) = self.sender.take() {
                sender.response_sent(self, ResponseSendResult::ResponseSent);
            }
        }
    }

    fn send_response(&'static self) -> ConnectionErrorCode {
        let (Some(message_sender), Some(response)) =
            (self.message_sender.get(), self.response_message.get())
        else {
            return ConnectionErrorCode::NoTpMessage;
        };
        if message_sender.send(response, Some(self)) == LayerErrorCode::Ok {
            return ConnectionErrorCode::Ok;
        }
        self.num_pending_message_processed_callbacks
            .set(self.num_pending_message_processed_callbacks.get() - 1);
        if let Some(sender) = self.sender.take() {
            sender.response_sent(self, ResponseSendResult::ResponseSendFailed);
        }
        if self.connection_termination_is_pending.get()
            && self.num_pending_message_processed_callbacks.get() == 0
        {
            self.terminate();
        }
        ConnectionErrorCode::SendFailed
    }

    fn async_transport_message_processed(&'static self) {
        let (message, status) = self.processed_args.get();
        let Some(message) = message else {
            return;
        };
        self.num_pending_message_processed_callbacks
            .set(self.num_pending_message_processed_callbacks.get() - 1);
        let is_pending_message = core::ptr::eq(message, &self.pending_message);
        if is_pending_message {
            self.response_pending_is_being_sent.set(false);
        }
        if let Some(sender) = self.sender.get() {
            if !is_pending_message {
                self.sender.set(None);
                sender.response_sent(
                    self,
                    if status == ProcessingResult::NoError {
                        ResponseSendResult::ResponseSent
                    } else {
                        ResponseSendResult::ResponseSendFailed
                    },
                );
            } else {
                // A response is pending and responsePending has been sent.
                let _ = self.send_response();
            }
        }
        if self.num_pending_message_processed_callbacks.get() == 0 {
            // All responses have been sent.
            if self.connection_termination_is_pending.get() {
                self.terminate();
            } else if self.response_pending_is_pending.get() {
                self.response_pending_is_pending.set(false);
                self.send_response_pending();
            }
        }
        if status != ProcessingResult::NoError {
            log_error!(
                UDS,
                b"IncomingDiagConnection::transportMessageSent(): failed to send message from 0x%x to 0x%x",
                message.source_id(),
                message.target_id()
            );
        }
    }

    /// A timer ran out: `global` is the request's overall limit, else the response-pending
    /// interval.
    pub fn expired(&'static self, global: bool) {
        if global {
            self.response_pending_timeout.timeout.cancel();
            self.terminate();
            return;
        }
        if self.num_pending_message_processed_callbacks.get() != 0 {
            // Don't send ResponsePending while a response is being sent.
            self.response_pending_is_pending.set(true);
        } else {
            self.response_pending_is_pending.set(false);
            self.send_response_pending();
        }
        self.restart_pending_timeout();
    }

    /// Stop the response-pending answers.
    pub fn disable_response_timeout(&'static self) {
        self.response_pending_timeout.timeout.cancel();
    }

    /// Stop the request's overall limit.
    pub fn disable_global_timeout(&'static self) {
        self.global_pending_timeout.timeout.cancel();
    }

    /// The positive response to this request is not sent (the request asked for that).
    pub fn suppress_positive_response(&self) {
        self.suppress_positive_response.set(true);
    }

    fn restart_pending_timeout(&'static self) {
        self.response_pending_timeout.timeout.cancel();
        if self.pending_activated.get() {
            openbsw_async::schedule(
                self.context.get(),
                &self.response_pending_timeout,
                &self.response_pending_timeout.timeout,
                self.pending_timeout_ms.get(),
                TimeUnit::Milliseconds,
            );
        }
    }

    fn send_response_pending(&'static self) {
        let Some(message_sender) = self.message_sender.get() else {
            return;
        };
        if !self.open.get() {
            return;
        }
        let pending = &self.pending_message;
        pending.set_target_address(self.source_id.get());
        self.set_source_id(pending);
        pending.reset_valid_bytes();
        let _ = pending.append_byte(DiagReturnCode::NEGATIVE_RESPONSE_IDENTIFIER);
        let _ = pending.append_byte(self.service_id.get());
        let _ = pending.append_byte(DiagReturnCode::IsoResponsePending.as_byte());
        pending.set_payload_length(Self::PENDING_MESSAGE_PAYLOAD_LENGTH);
        self.num_pending_message_processed_callbacks
            .set(self.num_pending_message_processed_callbacks.get() + 1);
        let response_pending_sent = self.response_pending_sent.get();
        let response_pending_is_being_sent = self.response_pending_is_being_sent.get();
        self.response_pending_sent.set(true);
        self.response_pending_is_being_sent.set(true);
        let result = message_sender.send(pending, Some(self));
        if result != LayerErrorCode::Ok {
            self.num_pending_message_processed_callbacks
                .set(self.num_pending_message_processed_callbacks.get() - 1);
            self.response_pending_sent.set(response_pending_sent);
            self.response_pending_is_being_sent.set(response_pending_is_being_sent);
            log_error!(
                UDS,
                b"IncomingDiagConnection: Unable to send ResponsePending: sendResult = %d!",
                layer_error_number(result)
            );
        }
    }

    fn set_source_id(&self, message: &TransportMessage) {
        if self.is_functionally_addressed() {
            let Some(manager) = self.connection_manager.get() else {
                log_critical!(
                    UDS,
                    b"IncomingDiagConnection::setSourceId(): fpDiagConnectionManager == nullptr!"
                );
                panic!("diagnostic connection manager must not be null");
            };
            message.set_source_address(manager.source_diag_id());
        } else {
            message.set_source_address(self.target_id.get());
        }
    }
}

impl Default for IncomingDiagConnection {
    fn default() -> Self {
        Self::new()
    }
}

impl TransportMessageProcessedListener for IncomingDiagConnection {
    fn transport_message_processed(
        &self,
        message: &'static TransportMessage,
        result: ProcessingResult,
    ) {
        let this = self.as_static();
        this.processed_args.set((Some(message), result));
        openbsw_async::execute(this.context.get(), &this.transport_message_processed_call);
    }
}
