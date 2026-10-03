// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Service 0x10 and the session manager: the port of
//! `services/sessioncontrol/DiagnosticSessionControl.h`.

use core::cell::Cell;

use openbsw_async::{ContextType, QueueNode, Runnable, TimeUnit, Timeout};
use openbsw_util::{log_debug, log_error, log_warn};

use crate::UDS;
use crate::codes::{DiagReturnCode, ServiceId};
use crate::connection::IncomingDiagConnection;
use crate::dispatcher::DispatcherControl;
use crate::job::{DiagJob, JobBase, Request, ResponseSendResult, Service, request_byte};
use crate::lifecycle::{ShutdownType, UdsLifecycleConnector};
use crate::persistence::SessionPersistence;
use crate::session::{
    APPLICATION_DEFAULT_SESSION, APPLICATION_EXTENDED_SESSION, DiagSession,
    DiagSessionChangedListener, DiagSessionManager, DiagSessionMask, PROGRAMMING_SESSION,
    SessionType,
};
use crate::uds_config::TESTER_PRESENT_TIMEOUT_MS;

/// Called once the persisted session was read at startup (`InitCompleteCallbackType`).
pub type InitCompleteCallback = &'static (dyn Fn() + Sync);

/// DiagnosticSessionControl: switches sessions on request, times the non-default ones
/// out, and is the session manager every job asks.
pub struct DiagnosticSessionControl {
    service: Service,
    context: ContextType,
    lifecycle_connector: &'static dyn UdsLifecycleConnector,
    persistence: &'static dyn SessionPersistence,
    dispatcher: Cell<Option<&'static dyn DispatcherControl>>,
    current_session: Cell<&'static DiagSession>,
    tester_present: Cell<Option<&'static JobBase>>,
    read_data_by_identifier: Cell<Option<&'static JobBase>>,
    ecu_reset: Cell<Option<&'static JobBase>>,
    diagnostic_session_control: Cell<Option<&'static JobBase>>,
    init_complete: Cell<Option<InitCompleteCallback>>,
    next_session_timeout: Cell<u32>,
    initializing: Cell<bool>,
    request_programming_session: Cell<bool>,
    tester_present_received: Cell<bool>,
    timeout: Timeout,
    is_active: Cell<bool>,
    listeners: Cell<Option<&'static dyn DiagSessionChangedListener>>,
    node: QueueNode<dyn Runnable>,
}

// SAFETY: the session and its timer change on the diagnostic context; listeners are added
// under the platform lock.
unsafe impl Sync for DiagnosticSessionControl {}

impl DiagnosticSessionControl {
    const RESET_TIME: u32 = 1000;
    const DEFAULT_DIAG_RESPONSE_TIME: u16 = 50;
    const DEFAULT_DIAG_RESPONSE_PENDING: u16 = 500;
    const EXTENDED_DIAG_RESPONSE_PENDING: u16 = 3000;

    /// A session control on `context`, asking `lifecycle_connector` for resets and
    /// `persistence` for the session across them.
    pub const fn new(
        lifecycle_connector: &'static dyn UdsLifecycleConnector,
        context: ContextType,
        persistence: &'static dyn SessionPersistence,
    ) -> Self {
        Self {
            service: Service::new(
                ServiceId::DIAGNOSTIC_SESSION_CONTROL,
                DiagSessionMask::ALL_SESSIONS,
            )
            .with_suppress_positive_response(),
            context,
            lifecycle_connector,
            persistence,
            dispatcher: Cell::new(None),
            current_session: Cell::new(&APPLICATION_DEFAULT_SESSION),
            tester_present: Cell::new(None),
            read_data_by_identifier: Cell::new(None),
            ecu_reset: Cell::new(None),
            diagnostic_session_control: Cell::new(None),
            init_complete: Cell::new(None),
            next_session_timeout: Cell::new(TESTER_PRESENT_TIMEOUT_MS),
            initializing: Cell::new(false),
            request_programming_session: Cell::new(false),
            tester_present_received: Cell::new(false),
            timeout: Timeout::new(),
            is_active: Cell::new(false),
            listeners: Cell::new(None),
            node: QueueNode::new(),
        }
    }

    /// Name the jobs the session logic refers to and read the persisted session; the demo
    /// does not call this.
    pub fn init(
        &'static self,
        read_data_by_identifier: &'static dyn DiagJob,
        ecu_reset: &'static dyn DiagJob,
        diagnostic_session_control: &'static dyn DiagJob,
        tester_present: &'static dyn DiagJob,
        init_complete: Option<InitCompleteCallback>,
    ) {
        self.init_complete.set(init_complete);
        self.tester_present.set(Some(tester_present.base()));
        self.diagnostic_session_control.set(Some(diagnostic_session_control.base()));
        self.read_data_by_identifier.set(Some(read_data_by_identifier.base()));
        self.ecu_reset.set(Some(ecu_reset.base()));
        self.initializing.set(true);
        self.persistence.read_session(self);
    }

    /// Stop the session timeout.
    pub fn shutdown(&'static self) {
        self.timeout.cancel();
        self.is_active.set(false);
    }

    /// The dispatcher to switch off for a programming session (`setDiagDispatcher`).
    pub fn set_diag_dispatcher(&self, dispatcher: Option<&'static dyn DispatcherControl>) {
        self.dispatcher.set(dispatcher);
    }

    /// The persisted session was read (`sessionRead`): start in the default session.
    pub fn session_read(&'static self, _session: u8) {
        log_debug!(
            UDS,
            b"Starting up with Session = 0x%x",
            self.current_session.get().session_byte()
        );
        self.switch_session(&APPLICATION_DEFAULT_SESSION);
        if let Some(callback) = self.init_complete.get() {
            callback();
        }
        self.initializing.set(false);
    }

    /// The session was written (`sessionWritten`): reset if it was.
    pub fn session_written(&self, successful: bool) {
        if !successful {
            log_error!(UDS, b"Writing session to eeprom failed!");
        } else {
            let _ = self
                .lifecycle_connector
                .request_shutdown(ShutdownType::HardReset, Self::RESET_TIME);
        }
    }

    fn set_timeout(&'static self, timeout_ms: u32) {
        self.timeout.cancel();
        self.is_active.set(true);
        openbsw_async::schedule(
            self.context,
            self,
            &self.timeout,
            timeout_ms,
            TimeUnit::Milliseconds,
        );
    }

    fn start_timeout(&'static self) {
        let current = self.current_session.get();
        if *current == APPLICATION_EXTENDED_SESSION || *current == PROGRAMMING_SESSION {
            self.set_timeout(self.next_session_timeout.get());
            self.next_session_timeout.set(TESTER_PRESENT_TIMEOUT_MS);
        }
    }

    fn stop_timeout(&'static self) {
        self.timeout.cancel();
        // As the C++: stopping leaves the flag set.
        self.is_active.set(true);
    }

    fn switch_session(&'static self, new_session: &'static DiagSession) {
        let old_session_byte = self.current_session.get().session_byte();
        if *new_session == APPLICATION_DEFAULT_SESSION {
            self.current_session.set(new_session);
            new_session.enter();
            self.stop_timeout();
        } else if *new_session == APPLICATION_EXTENDED_SESSION {
            if !core::ptr::eq(self.current_session.get(), new_session) {
                self.current_session.set(new_session);
                new_session.enter();
            }
            self.set_timeout(TESTER_PRESENT_TIMEOUT_MS);
        } else if *new_session == PROGRAMMING_SESSION {
            if let Some(dispatcher) = self.dispatcher.get() {
                dispatcher.disable();
            }
            self.request_programming_session.set(true);
        } else {
            self.current_session.set(new_session);
            new_session.enter();
        }
        log_debug!(
            UDS,
            b"switching from session 0x%x to 0x%x ",
            old_session_byte,
            new_session.session_byte()
        );
        let mut current = self.listeners.get();
        while let Some(listener) = current {
            listener.diag_session_changed(self.current_session.get());
            current = listener.node().next.get();
        }
    }

    fn expired(&'static self) {
        log_warn!(
            UDS,
            b"Session timeout in session 0x%x",
            self.current_session.get().session_byte()
        );
        if *self.current_session.get() == APPLICATION_EXTENDED_SESSION {
            self.switch_session(&APPLICATION_DEFAULT_SESSION);
        } else {
            log_error!(
                UDS,
                b"Session timeout in session 0x%x is NOT allowed!",
                self.current_session.get().session_byte()
            );
        }
    }

    fn as_static(&self) -> &'static Self {
        // SAFETY: the session control is a `static` of the UDS system, as the C++ object is
        // a member of the system that lives as long as the program.
        unsafe { &*(self as *const Self) }
    }
}

impl Runnable for DiagnosticSessionControl {
    fn execute(&self) {
        self.as_static().expired();
    }

    fn node(&self) -> &QueueNode<dyn Runnable> {
        &self.node
    }
}

impl DiagJob for DiagnosticSessionControl {
    fn base(&self) -> &JobBase {
        self.service.base()
    }

    fn verify(&self, request: Request<'_>) -> DiagReturnCode {
        self.service.verify(request)
    }

    fn process(
        &'static self,
        connection: &'static IncomingDiagConnection,
        request: Request<'_>,
    ) -> DiagReturnCode {
        if request.len() != 1 {
            return DiagReturnCode::IsoInvalidFormat;
        }
        let current = self.current_session.get();
        let requested = request_byte(request, 0);
        log_debug!(UDS, b"%d -> %d", current.session_type() as u8, requested);
        let Some(requested_session) = SessionType::from_byte(requested) else {
            // The C++ switch on an unknown session type falls through to subFunctionNotSupported.
            return DiagReturnCode::IsoSubfunctionNotSupported;
        };
        let switch_session_result = current.is_transition_possible(requested_session);
        if switch_session_result != DiagReturnCode::Ok {
            return switch_session_result;
        }
        connection.add_identifier();
        let response = connection.release_request_get_response();
        if response.maximum_length() < 4 {
            return DiagReturnCode::IsoResponseTooLong;
        }
        if requested_session == SessionType::Programming {
            let _ = response.append_u16(Self::DEFAULT_DIAG_RESPONSE_TIME);
            let _ = response.append_u16(Self::EXTENDED_DIAG_RESPONSE_PENDING);
            log_debug!(UDS, b"EXTENDED TIMEOUTS");
        } else {
            let _ = response.append_u16(Self::DEFAULT_DIAG_RESPONSE_TIME);
            let _ = response.append_u16(Self::DEFAULT_DIAG_RESPONSE_PENDING);
            log_debug!(UDS, b"DEFAULT SESSION TIMEOUTS");
        }
        let new_session = current.transition_result(requested_session);
        self.switch_session(new_session);
        let _ = connection.send_positive_response_internal(response.length() as u16, self);
        log_debug!(UDS, b"Active Session 0x%x", self.current_session.get().session_byte());
        DiagReturnCode::Ok
    }

    fn response_sent(
        &'static self,
        connection: &'static IncomingDiagConnection,
        _result: ResponseSendResult,
    ) {
        connection.terminate();
        if self.request_programming_session.get() {
            self.request_programming_session.set(false);
            self.persistence.write_session(self, SessionType::Programming as u8);
        }
    }
}

impl DiagSessionManager for DiagnosticSessionControl {
    fn active_session(&self) -> &'static DiagSession {
        self.current_session.get()
    }

    fn start_session_timeout(&'static self) {
        self.start_timeout();
    }

    fn stop_session_timeout(&'static self) {
        self.stop_timeout();
    }

    fn is_session_timeout_active(&self) -> bool {
        self.is_active.get()
    }

    fn reset_to_default_session(&'static self) {
        let new_session = self.current_session.get().transition_result(SessionType::Default);
        self.switch_session(new_session);
    }

    fn accepted_job(
        &'static self,
        _connection: &IncomingDiagConnection,
        job: &JobBase,
        _request: Request<'_>,
    ) -> DiagReturnCode {
        log_debug!(
            UDS,
            b"Accepted 0x%x, current session: 0x%x",
            job.request_id(),
            self.active_session().session_byte()
        );
        self.tester_present_received.set(
            self.tester_present
                .get()
                .is_some_and(|tester_present| core::ptr::eq(tester_present, job)),
        );
        self.stop_session_timeout();
        DiagReturnCode::Ok
    }

    fn response_sent(
        &'static self,
        _connection: &IncomingDiagConnection,
        result: DiagReturnCode,
        _response: Request<'_>,
    ) {
        log_debug!(
            UDS,
            b"Sent response 0x%x, tp %d",
            result,
            u32::from(self.tester_present_received.get())
        );
        if result != DiagReturnCode::IsoResponsePending {
            self.start_session_timeout();
        }
        if self.tester_present_received.get() {
            // Do nothing.
            self.tester_present_received.set(false);
        } else {
            // Notify.
            let mut current = self.listeners.get();
            while let Some(listener) = current {
                listener.diag_session_response_sent(result.as_byte());
                current = listener.node().next.get();
            }
        }
    }

    fn add_diag_session_listener(&self, listener: &'static dyn DiagSessionChangedListener) {
        let mut current = self.listeners.get();
        let mut last: Option<&'static dyn DiagSessionChangedListener> = None;
        while let Some(node) = current {
            last = Some(node);
            current = node.node().next.get();
        }
        listener.node().next.set(None);
        match last {
            None => self.listeners.set(Some(listener)),
            Some(last) => last.node().next.set(Some(listener)),
        }
    }

    fn remove_diag_session_listener(&self, listener: &'static dyn DiagSessionChangedListener) {
        let mut prev: Option<&'static dyn DiagSessionChangedListener> = None;
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
}
