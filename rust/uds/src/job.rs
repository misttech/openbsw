// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The tree of diagnostic jobs, ported from `base/AbstractDiagJob.h`, `DiagJobRoot.h`,
//! `Service.h` and `Subfunction.h`.
//!
//! A job matches a prefix of the request (its "implemented request"), verifies the rest,
//! and either answers or hands what follows to its children. The C++ classes inherit from
//! `AbstractDiagJob`; here every job holds a [`JobBase`] and implements [`DiagJob`], whose
//! provided methods are the base class's algorithm.

use core::cell::Cell;

use openbsw_util::cell::RacyCell;
use openbsw_util::log_debug;

use crate::UDS;
use crate::authenticator::{DEFAULT_DIAG_AUTHENTICATOR, DiagAuthenticator};
use crate::codes::DiagReturnCode;
use crate::connection::IncomingDiagConnection;
use crate::session::{DiagSessionManager, DiagSessionMask};

/// A request, as a window on the transport message's payload cells: the C++ clears the
/// suppress-positive-response bit in place, and so does this port.
pub type Request<'a> = &'a [Cell<u8>];

/// The byte at `index` of `request`.
pub fn request_byte(request: Request<'_>, index: usize) -> u8 {
    request[index].get()
}

/// The longest implemented request a job stores: a service, a subfunction and a two-byte id.
pub const MAX_IMPLEMENTED_REQUEST_LENGTH: usize = 4;

/// A job without a request (`EMPTY_REQUEST`).
pub const EMPTY_REQUEST: u8 = 0;
/// A job without a response (`EMPTY_RESPONSE`).
pub const EMPTY_RESPONSE: u8 = 0;
/// Any request length (`VARIABLE_REQUEST_LENGTH`).
pub const VARIABLE_REQUEST_LENGTH: u8 = 0xFF;
/// Any response length (`VARIABLE_RESPONSE_LENGTH`).
pub const VARIABLE_RESPONSE_LENGTH: u8 = 0xFF;

const SUPPRESS_POSITIVE_RESPONSE_MASK: u8 = 0x80;

/// Whether a job was added to the tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobErrorCode {
    /// Added.
    JobAdded,
    /// Not added: already there, or no parent for it.
    JobNotAdded,
}

/// How sending a response ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponseSendResult {
    /// Sent.
    ResponseSent,
    /// Not sent.
    ResponseSendFailed,
}

static DEFAULT_SESSION_MANAGER: RacyCell<Option<&'static dyn DiagSessionManager>> =
    RacyCell::new(None);
static DIAG_JOB_ROOT: RacyCell<Option<&'static DiagJobRoot>> = RacyCell::new(None);

/// Install the session manager jobs use unless they have their own
/// (`AbstractDiagJob::setDefaultDiagSessionManager`).
pub fn set_default_diag_session_manager(manager: Option<&'static dyn DiagSessionManager>) {
    // SAFETY: called while the UDS system initializes, before requests are dispatched.
    unsafe { DEFAULT_SESSION_MANAGER.set(manager) };
}

/// The installed session manager.
///
/// # Panics
///
/// When none is installed, as the C++ asserts.
pub fn default_diag_session_manager() -> &'static dyn DiagSessionManager {
    // SAFETY: set during initialization, read afterwards.
    (*unsafe { DEFAULT_SESSION_MANAGER.get() }).expect("session manager must not be null")
}

/// Name the root of the job tree (the C++ root constructor registers itself).
pub fn set_diag_job_root(root: Option<&'static DiagJobRoot>) {
    // SAFETY: called while the UDS system initializes, before requests are dispatched.
    unsafe { DIAG_JOB_ROOT.set(root) };
}

fn diag_job_root() -> Option<&'static DiagJobRoot> {
    // SAFETY: set during initialization, read afterwards.
    *unsafe { DIAG_JOB_ROOT.get() }
}

/// What every job carries: its implemented request, lengths, allowed sessions, default
/// answer, and its place in the tree.
pub struct JobBase {
    implemented_request: Option<[u8; MAX_IMPLEMENTED_REQUEST_LENGTH]>,
    request_length: u8,
    prefix_length: u8,
    request_payload_length: u8,
    response_length: u8,
    allowed_sessions: DiagSessionMask,
    default_return_code: Cell<DiagReturnCode>,
    suppress_positive_response_enabled: Cell<bool>,
    first_child: Cell<Option<&'static dyn DiagJob>>,
    next_job: Cell<Option<&'static dyn DiagJob>>,
}

// SAFETY: the tree links change while jobs are added and removed, during lifecycle
// transitions; the flags are set while the system is built.
unsafe impl Sync for JobBase {}

impl JobBase {
    /// A job matching `implemented_request` (`None` for the root, which matches nothing
    /// itself), of which the first `prefix_length` bytes its parent already matched.
    ///
    /// # Panics
    ///
    /// When `request_length` is neither zero nor more than `prefix_length`.
    pub const fn new(
        implemented_request: Option<&[u8]>,
        request_length: u8,
        prefix_length: u8,
        allowed_sessions: DiagSessionMask,
    ) -> Self {
        Self::with_lengths(
            implemented_request,
            request_length,
            prefix_length,
            VARIABLE_REQUEST_LENGTH,
            VARIABLE_RESPONSE_LENGTH,
            allowed_sessions,
        )
    }

    /// As [`new`](Self::new), with a fixed payload length after the implemented request and
    /// a fixed response length (`VARIABLE_*` for either means any).
    pub const fn with_lengths(
        implemented_request: Option<&[u8]>,
        request_length: u8,
        prefix_length: u8,
        request_payload_length: u8,
        response_length: u8,
        allowed_sessions: DiagSessionMask,
    ) -> Self {
        assert!(
            request_length == 0 || request_length > prefix_length,
            "requested length must be greater than the prefix length"
        );
        let implemented_request = match implemented_request {
            None => None,
            Some(bytes) => {
                assert!(
                    bytes.len() <= MAX_IMPLEMENTED_REQUEST_LENGTH,
                    "an implemented request has at most four bytes"
                );
                let mut stored = [0u8; MAX_IMPLEMENTED_REQUEST_LENGTH];
                let mut i = 0;
                while i < bytes.len() {
                    stored[i] = bytes[i];
                    i += 1;
                }
                Some(stored)
            }
        };
        Self {
            implemented_request,
            request_length,
            prefix_length,
            request_payload_length,
            response_length,
            allowed_sessions,
            default_return_code: Cell::new(DiagReturnCode::IsoGeneralReject),
            suppress_positive_response_enabled: Cell::new(false),
            first_child: Cell::new(None),
            next_job: Cell::new(None),
        }
    }

    /// This base with `code` as the answer when no child is responsible.
    pub const fn with_default_return_code(self, code: DiagReturnCode) -> Self {
        Self { default_return_code: Cell::new(code), ..self }
    }

    /// This base with the suppress-positive-response bit honored.
    pub const fn with_suppress_positive_response(self) -> Self {
        Self { suppress_positive_response_enabled: Cell::new(true), ..self }
    }

    /// The implemented request's first `request_length` bytes, `None` for the root.
    pub fn implemented_request(&self) -> Option<&[u8]> {
        self.implemented_request
            .as_ref()
            .map(|bytes| &bytes[..usize::from(self.request_length).min(bytes.len())])
    }

    /// The implemented request's length.
    pub fn request_length(&self) -> u8 {
        self.request_length
    }

    /// How many of its bytes the parent matched.
    pub fn prefix_length(&self) -> u8 {
        self.prefix_length
    }

    /// The sessions this job answers in.
    pub fn allowed_sessions(&self) -> DiagSessionMask {
        self.allowed_sessions
    }

    /// The answer when no child is responsible.
    pub fn default_return_code(&self) -> DiagReturnCode {
        self.default_return_code.get()
    }

    /// Set the answer when no child is responsible.
    pub fn set_default_return_code(&self, code: DiagReturnCode) {
        self.default_return_code.set(code);
    }

    /// Honor (or not) the suppress-positive-response bit of the request's first byte.
    pub fn enable_suppress_positive_response(&self, set: bool) {
        self.suppress_positive_response_enabled.set(set);
    }

    /// Copy this job's suppress-positive-response setting to `job`.
    pub fn set_enable_suppress_positive_response(&self, job: &JobBase) {
        job.suppress_positive_response_enabled.set(self.suppress_positive_response_enabled.get());
    }

    /// The implemented request's first bytes as a number, for the log (`getRequestId`).
    pub fn request_id(&self) -> u32 {
        let mut id = 0u32;
        if let Some(bytes) = self.implemented_request() {
            for &byte in bytes.iter().take(4) {
                id = (id << 8) + u32::from(byte);
            }
        }
        id
    }

    /// The next job at this level of the tree.
    pub fn next_job(&self) -> Option<&'static dyn DiagJob> {
        self.next_job.get()
    }

    /// The first child.
    pub fn first_child(&self) -> Option<&'static dyn DiagJob> {
        self.first_child.get()
    }

    /// Link `job` after this one, unless it is this job or one just like it.
    pub fn set_next_job(&self, job: Option<&'static dyn DiagJob>) {
        if let Some(job) = job
            && (core::ptr::eq(job.base(), self) || jobs_equal(job.base(), self))
        {
            log_debug!(
                UDS,
                b"Tried to add AbstractDiagJob as next job to itself 0x%X",
                self.request_id()
            );
            return;
        }
        self.next_job.set(job);
    }

    /// Whether `prefix` (the parent's implemented request) is exactly this job's prefix.
    pub fn is_child(&self, prefix: Option<&[u8]>) -> bool {
        let length = prefix.map_or(0, <[u8]>::len);
        length == usize::from(self.prefix_length)
            && compare(prefix, self.implemented_request.as_ref().map(|bytes| &bytes[..]), length)
    }

    /// Whether `prefix` is a prefix of this job's prefix: the job belongs below it.
    pub fn is_family(&self, prefix: Option<&[u8]>) -> bool {
        let length = prefix.map_or(0, <[u8]>::len);
        length <= usize::from(self.prefix_length)
            && compare(prefix, self.implemented_request.as_ref().map(|bytes| &bytes[..]), length)
    }

    /// Add `job` below this one (`addAbstractDiagJob`): as a child when this job's request
    /// is its prefix, else through the children it belongs under.
    pub fn add_abstract_diag_job(&self, job: &'static dyn DiagJob) -> JobErrorCode {
        // This is an extra check intended not to print a message, because every job meets
        // itself once during the add process.
        if core::ptr::eq(job.base(), self) {
            return JobErrorCode::JobNotAdded;
        }
        // Whereas this check assures that different instances are not semantically equal.
        if jobs_equal(job.base(), self)
            || self.first_child.get().is_some_and(|child| core::ptr::eq(child.base(), job.base()))
        {
            log_debug!(
                UDS,
                b"AbstractDiagJob::add() tried to add identical jobs 0x%X",
                job.base().request_id()
            );
            return JobErrorCode::JobNotAdded;
        }
        if job.base().is_child(self.implemented_request()) {
            // This job belongs to me.
            match self.first_child.get() {
                None => self.first_child.set(Some(job)),
                Some(first) => {
                    let mut current = first;
                    loop {
                        if core::ptr::eq(current.base(), job.base())
                            || jobs_equal(current.base(), job.base())
                        {
                            log_debug!(
                                UDS,
                                b"AbstractDiagJob::add() tried to add identical jobs 0x%X",
                                job.base().request_id()
                            );
                            return JobErrorCode::JobNotAdded;
                        }
                        match current.base().next_job() {
                            Some(next) => current = next,
                            None => break,
                        }
                    }
                    current.base().set_next_job(Some(job));
                }
            }
            job.base().next_job.set(None);
            job.base().first_child.set(None);
            return JobErrorCode::JobAdded;
        }
        if job.base().is_family(self.implemented_request()) {
            // Ask my children.
            let mut current = self.first_child.get();
            while let Some(child) = current {
                if child.base().add_abstract_diag_job(job) == JobErrorCode::JobAdded {
                    return JobErrorCode::JobAdded;
                }
                current = child.base().next_job();
            }
        }
        JobErrorCode::JobNotAdded
    }

    /// Take `job` out of the tree below this one (`removeAbstractDiagJob`).
    pub fn remove_abstract_diag_job(&self, job: &'static dyn DiagJob) {
        if core::ptr::eq(job.base(), self) {
            // We cannot remove us from ourself.
            return;
        }
        if self.first_child.get().is_some_and(|child| core::ptr::eq(child.base(), job.base())) {
            // We remove our first child.
            self.first_child.set(job.base().next_job());
            return;
        }
        if let Some(child) = self.first_child.get() {
            child.base().remove_abstract_diag_job(job);
        }
        if self.next_job.get().is_some_and(|next| core::ptr::eq(next.base(), job.base())) {
            self.set_next_job(job.base().next_job());
            return;
        }
        if let Some(next) = self.next_job.get() {
            next.base().remove_abstract_diag_job(job);
        }
    }

    /// Ask the children in turn (`AbstractDiagJob::process`): the first responsible one
    /// answers, else the default return code.
    pub fn process_children(
        &self,
        connection: &'static IncomingDiagConnection,
        request: Request<'_>,
    ) -> DiagReturnCode {
        let mut result = DiagReturnCode::NotResponsible;
        let mut current = self.first_child.get();
        while result == DiagReturnCode::NotResponsible
            && let Some(child) = current
        {
            result = child.execute(connection, request);
            current = child.base().next_job();
        }
        if result == DiagReturnCode::NotResponsible {
            return self.default_return_code.get();
        }
        result
    }

    fn accept_job(
        &self,
        manager: &'static dyn DiagSessionManager,
        connection: &IncomingDiagConnection,
        request: Request<'_>,
    ) {
        if self.request_length > 0 {
            let _ = manager.accepted_job(connection, self, request);
        }
    }

    fn check_suppress_positive_response_bit(
        &self,
        connection: &IncomingDiagConnection,
        request: Request<'_>,
    ) {
        if self.suppress_positive_response_enabled.get()
            && let Some(first) = request.first()
            && first.get() & SUPPRESS_POSITIVE_RESPONSE_MASK != 0
        {
            // Remove the suppressPositiveResponse bit.
            first.set(first.get() & 0x7F);
            connection.suppress_positive_response();
        }
    }
}

/// Whether two jobs implement the same request (`operator==`); jobs without implemented
/// requests never match.
pub fn jobs_equal(x: &JobBase, y: &JobBase) -> bool {
    match (x.implemented_request, y.implemented_request) {
        (Some(a), Some(b)) => {
            x.request_length == y.request_length
                && x.prefix_length == y.prefix_length
                && compare(Some(&a), Some(&b), usize::from(x.request_length))
        }
        _ => false,
    }
}

/// `AbstractDiagJob::compare`: `length` bytes are equal; two absent arrays are equal, one is
/// not; nothing is compared for `length` zero.
pub fn compare(data1: Option<&[u8]>, data2: Option<&[u8]>, length: usize) -> bool {
    if length == 0 {
        return true;
    }
    match (data1, data2) {
        (None, None) => true,
        (Some(a), Some(b)) => a.get(..length) == b.get(..length) && a.len() >= length,
        _ => false,
    }
}

/// A diagnostic job: the port of `AbstractDiagJob`. The required methods are what a C++
/// subclass overrides; the provided ones are the base class.
pub trait DiagJob: Sync {
    /// The job's base data.
    fn base(&self) -> &JobBase;

    /// Whether `request` (what follows the parent's prefix) is this job's: `Ok`,
    /// `NotResponsible`, or why it is malformed.
    fn verify(&self, request: Request<'_>) -> DiagReturnCode;

    /// Answer `request` (what follows this job's implemented request) on `connection`; the
    /// default asks the children.
    fn process(
        &'static self,
        connection: &'static IncomingDiagConnection,
        request: Request<'_>,
    ) -> DiagReturnCode {
        self.base().process_children(connection, request)
    }

    /// The response this job sent went out; the default closes the connection.
    fn response_sent(
        &'static self,
        connection: &'static IncomingDiagConnection,
        _result: ResponseSendResult,
    ) {
        connection.terminate();
    }

    /// Who checks the tester's address; the default accepts everyone.
    fn authenticator(&self) -> &dyn DiagAuthenticator {
        &DEFAULT_DIAG_AUTHENTICATOR
    }

    /// The session manager; the default is the installed one.
    fn session_manager(&self) -> &'static dyn DiagSessionManager {
        default_diag_session_manager()
    }

    /// Run `request` through this job (`AbstractDiagJob::execute`): verify it, check the
    /// session, the tester, the lengths, then process it.
    fn execute(
        &'static self,
        connection: &'static IncomingDiagConnection,
        request: Request<'_>,
    ) -> DiagReturnCode {
        let base = self.base();
        let manager = self.session_manager();
        let mut status = self.verify(request);
        let root = diag_job_root();
        if status == DiagReturnCode::Ok {
            if !base.allowed_sessions.matches(manager.active_session()) {
                base.accept_job(manager, connection, request);
                return DiagReturnCode::IsoRequestOutOfRange;
            }
            if !self.authenticator().is_authenticated(connection.source_address()) {
                base.accept_job(manager, connection, request);
                return self.authenticator().not_authenticated_return_code();
            }
            if let Some(root) = root {
                let vsistat = root.verify_supplier_indication(request);
                if vsistat != DiagReturnCode::Ok {
                    base.accept_job(manager, connection, request);
                    return vsistat;
                }
            }
            let own = usize::from(base.request_length) - usize::from(base.prefix_length);
            if base.request_payload_length != VARIABLE_REQUEST_LENGTH {
                let payload_length = request.len().wrapping_sub(own);
                if payload_length != usize::from(base.request_payload_length) {
                    base.accept_job(manager, connection, request);
                    return DiagReturnCode::IsoInvalidFormat;
                }
            }
            for _ in 0..own {
                connection.add_identifier();
            }
            if base.response_length != VARIABLE_RESPONSE_LENGTH
                && connection.maximum_response_length() < u16::from(base.response_length)
            {
                base.accept_job(manager, connection, request);
                return DiagReturnCode::IsoResponseTooLong;
            }
            let rest = &request[own.min(request.len())..];
            base.check_suppress_positive_response_bit(connection, rest);
            base.accept_job(manager, connection, rest);
            log_debug!(UDS, b"Process diag job 0x%X", base.request_id());
            return self.process(connection, rest);
        }
        if status != DiagReturnCode::NotResponsible {
            if let Some(root) = root
                && status != DiagReturnCode::IsoServiceNotSupported
                && status != DiagReturnCode::IsoServiceNotSupportedInActiveSession
                && status != DiagReturnCode::IsoSecurityAccessDenied
            {
                let vsistat = root.verify_supplier_indication(request);
                if vsistat != DiagReturnCode::Ok {
                    status = vsistat;
                }
            }
            let _ = manager.accepted_job(connection, base, request);
        }
        status
    }
}

/// The root of the tree: the port of `DiagJobRoot`. Services hang below it; it refuses
/// empty requests and answers `serviceNotSupported` when none of them is responsible.
pub struct DiagJobRoot {
    base: JobBase,
}

impl DiagJobRoot {
    /// A root with no children.
    pub const fn new() -> Self {
        Self {
            base: JobBase::new(None, 0, 0, DiagSessionMask::ALL_SESSIONS)
                .with_default_return_code(DiagReturnCode::IsoServiceNotSupported),
        }
    }

    /// Run `request` from a tester (`DiagJobRoot::execute`): no answer to an incoming
    /// negative response, nor to a functionally addressed TesterPresent that suppresses
    /// its answer while no session timeout runs; otherwise down the tree.
    pub fn execute(
        &'static self,
        connection: &'static IncomingDiagConnection,
        request: Request<'_>,
    ) -> DiagReturnCode {
        if connection.service_id() == DiagReturnCode::NEGATIVE_RESPONSE_IDENTIFIER {
            // No response to an incoming NRC.
            connection.terminate();
            return DiagReturnCode::Ok;
        }
        let manager = self.session_manager();
        if connection.is_functionally_addressed()
            && connection.service_id() == crate::codes::ServiceId::TESTER_PRESENT
            && !manager.is_session_timeout_active()
            && request.len() > 1
            && request_byte(request, 1) & SUPPRESS_POSITIVE_RESPONSE_MASK != 0
        {
            connection.terminate();
            return DiagReturnCode::Ok;
        }
        let ret = self.verify(request);
        if ret != DiagReturnCode::Ok {
            let _ = manager.accepted_job(connection, &self.base, request);
            return ret;
        }
        if !self.base.allowed_sessions.matches(manager.active_session()) {
            self.base.accept_job(manager, connection, request);
            return DiagReturnCode::IsoRequestOutOfRange;
        }
        self.process(connection, request)
    }

    /// A hook for a supplier's own checks; nothing is checked.
    pub fn verify_supplier_indication(&self, _request: Request<'_>) -> DiagReturnCode {
        DiagReturnCode::Ok
    }
}

impl Default for DiagJobRoot {
    fn default() -> Self {
        Self::new()
    }
}

impl DiagJob for DiagJobRoot {
    fn base(&self) -> &JobBase {
        &self.base
    }

    fn verify(&self, request: Request<'_>) -> DiagReturnCode {
        if request.is_empty() {
            // No empty requests!
            return DiagReturnCode::IsoGeneralReject;
        }
        DiagReturnCode::Ok
    }
}

/// A service: a job matching one service id (`Service`). Concrete services hold one and
/// implement [`DiagJob`] around it; it is a job itself for tests and plain services.
pub struct Service {
    base: JobBase,
}

impl Service {
    /// A service answering `service` in `allowed_sessions`, with any request and response
    /// length.
    pub const fn new(service: u8, allowed_sessions: DiagSessionMask) -> Self {
        Self::with_lengths(
            service,
            VARIABLE_REQUEST_LENGTH,
            VARIABLE_RESPONSE_LENGTH,
            allowed_sessions,
        )
    }

    /// A service answering `service` with fixed request payload and response lengths.
    pub const fn with_lengths(
        service: u8,
        request_payload_length: u8,
        response_length: u8,
        allowed_sessions: DiagSessionMask,
    ) -> Self {
        Self {
            base: JobBase::with_lengths(
                Some(&[service]),
                1,
                0,
                request_payload_length,
                response_length,
                allowed_sessions,
            )
            .with_default_return_code(DiagReturnCode::IsoSubfunctionNotSupported),
        }
    }

    /// This service with the suppress-positive-response bit honored.
    pub const fn with_suppress_positive_response(self) -> Self {
        Self { base: self.base.with_suppress_positive_response() }
    }

    /// This service with `code` as the answer when no child is responsible.
    pub const fn with_default_return_code(self, code: DiagReturnCode) -> Self {
        Self { base: self.base.with_default_return_code(code) }
    }

    /// The base.
    pub fn base(&self) -> &JobBase {
        &self.base
    }

    /// `Service::verify`: the service id must match, and the session must allow it.
    pub fn verify(&self, request: Request<'_>) -> DiagReturnCode {
        if request_byte(request, 0) != self.base.implemented_request.map_or(0, |bytes| bytes[0]) {
            return DiagReturnCode::NotResponsible;
        }
        if !self.base.allowed_sessions.matches(default_diag_session_manager().active_session()) {
            return DiagReturnCode::IsoServiceNotSupportedInActiveSession;
        }
        DiagReturnCode::Ok
    }
}

impl DiagJob for Service {
    fn base(&self) -> &JobBase {
        &self.base
    }

    fn verify(&self, request: Request<'_>) -> DiagReturnCode {
        Service::verify(self, request)
    }
}

/// A subfunction: a job matching a service id and a subfunction byte (`Subfunction`).
pub struct Subfunction {
    base: JobBase,
}

impl Subfunction {
    const MINIMUM_REQUEST_LENGTH: usize = 1;

    /// A subfunction for `implemented_request` (service, subfunction) in `allowed_sessions`.
    pub const fn new(implemented_request: &[u8; 2], allowed_sessions: DiagSessionMask) -> Self {
        Self::with_lengths(
            implemented_request,
            VARIABLE_REQUEST_LENGTH,
            VARIABLE_RESPONSE_LENGTH,
            allowed_sessions,
        )
    }

    /// A subfunction with fixed request payload and response lengths.
    pub const fn with_lengths(
        implemented_request: &[u8; 2],
        request_payload_length: u8,
        response_length: u8,
        allowed_sessions: DiagSessionMask,
    ) -> Self {
        Self {
            base: JobBase::with_lengths(
                Some(implemented_request),
                2,
                1,
                request_payload_length,
                response_length,
                allowed_sessions,
            ),
        }
    }

    /// This subfunction with `code` as the answer when no child is responsible.
    pub const fn with_default_return_code(self, code: DiagReturnCode) -> Self {
        Self { base: self.base.with_default_return_code(code) }
    }

    /// The base.
    pub fn base(&self) -> &JobBase {
        &self.base
    }

    /// `Subfunction::verify`: a byte must be there, match, and the session must allow it.
    pub fn verify(&self, request: Request<'_>) -> DiagReturnCode {
        if request.len() < Self::MINIMUM_REQUEST_LENGTH {
            return DiagReturnCode::IsoInvalidFormat;
        }
        if request_byte(request, 0) != self.base.implemented_request.map_or(0, |bytes| bytes[1]) {
            return DiagReturnCode::NotResponsible;
        }
        if !self.base.allowed_sessions.matches(default_diag_session_manager().active_session()) {
            return DiagReturnCode::IsoSubfunctionNotSupportedInActiveSession;
        }
        DiagReturnCode::Ok
    }
}

impl DiagJob for Subfunction {
    fn base(&self) -> &JobBase {
        &self.base
    }

    fn verify(&self, request: Request<'_>) -> DiagReturnCode {
        Subfunction::verify(self, request)
    }
}

// Ported from AbstractDiagJobTest.cpp, DiagJobRootTest.cpp, ServiceTest.cpp and
// SubfunctionTest.cpp. The session manager and the root are process-wide, so the tests
// run one at a time.
#[cfg(test)]
pub(crate) mod tests {
    extern crate std;

    use std::boxed::Box;
    use std::sync::Mutex;
    use std::vec::Vec;

    use super::*;
    use crate::session::{
        APPLICATION_DEFAULT_SESSION, APPLICATION_EXTENDED_SESSION, DiagSession,
        DiagSessionChangedListener,
    };

    pub(crate) static SERIAL: Mutex<()> = Mutex::new(());

    pub(crate) fn serial() -> std::sync::MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Request bytes as cells.
    pub(crate) fn cells(bytes: &[u8]) -> &'static [Cell<u8>] {
        Box::leak(bytes.iter().map(|&byte| Cell::new(byte)).collect::<Vec<_>>().into_boxed_slice())
    }

    /// A message over a fresh buffer of `capacity` cells.
    pub(crate) fn message(capacity: usize) -> &'static openbsw_transport::TransportMessage {
        Box::leak(Box::new(openbsw_transport::TransportMessage::with_buffer(cells(&std::vec![
            0;
            capacity
        ]))))
    }

    /// `DiagSessionManagerMock`: answers a chosen session and counts what it is told.
    pub(crate) struct MockSessionManager {
        pub(crate) session: Cell<&'static DiagSession>,
        pub(crate) timeout_active: Cell<bool>,
        pub(crate) accepted: Mutex<Vec<(u32, Vec<u8>)>>,
        pub(crate) responses: Mutex<Vec<DiagReturnCode>>,
    }

    // SAFETY: a test fixture.
    unsafe impl Sync for MockSessionManager {}

    impl MockSessionManager {
        pub(crate) const fn new() -> Self {
            Self {
                session: Cell::new(&APPLICATION_DEFAULT_SESSION),
                timeout_active: Cell::new(false),
                accepted: Mutex::new(Vec::new()),
                responses: Mutex::new(Vec::new()),
            }
        }

        pub(crate) fn reset(&self, session: &'static DiagSession) {
            self.session.set(session);
            self.timeout_active.set(false);
            self.accepted.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clear();
            self.responses.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clear();
        }

        pub(crate) fn accepted_ids(&self) -> Vec<u32> {
            self.accepted
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .iter()
                .map(|(id, _)| *id)
                .collect()
        }
    }

    impl DiagSessionManager for MockSessionManager {
        fn active_session(&self) -> &'static DiagSession {
            self.session.get()
        }

        fn start_session_timeout(&'static self) {
            self.timeout_active.set(true);
        }

        fn stop_session_timeout(&'static self) {
            self.timeout_active.set(false);
        }

        fn is_session_timeout_active(&self) -> bool {
            self.timeout_active.get()
        }

        fn reset_to_default_session(&'static self) {
            self.session.set(&APPLICATION_DEFAULT_SESSION);
        }

        fn accepted_job(
            &'static self,
            _connection: &IncomingDiagConnection,
            job: &JobBase,
            request: Request<'_>,
        ) -> DiagReturnCode {
            self.accepted
                .lock()
                .unwrap()
                .push((job.request_id(), request.iter().map(Cell::get).collect()));
            DiagReturnCode::Ok
        }

        fn response_sent(
            &'static self,
            _connection: &IncomingDiagConnection,
            result: DiagReturnCode,
            _response: Request<'_>,
        ) {
            self.responses.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(result);
        }

        fn add_diag_session_listener(&self, _listener: &'static dyn DiagSessionChangedListener) {}

        fn remove_diag_session_listener(&self, _listener: &'static dyn DiagSessionChangedListener) {
        }
    }

    pub(crate) static SESSION_MANAGER: MockSessionManager = MockSessionManager::new();

    /// A connection with a request message, as the tests' `IncomingDiagConnection` fixture.
    pub(crate) fn new_connection(
        request: &'static openbsw_transport::TransportMessage,
    ) -> &'static IncomingDiagConnection {
        let connection: &'static IncomingDiagConnection =
            Box::leak(Box::new(IncomingDiagConnection::new()));
        connection.bind();
        connection.set_addresses(0x10, 0x2A, request.service_id());
        connection.set_request_message(Some(request));
        connection
    }

    /// `TestableDiagJob`: a job accepting any non-empty request, with its own authenticator.
    pub(crate) struct TestableDiagJob {
        pub(crate) base: JobBase,
        pub(crate) authenticated: Cell<bool>,
    }

    // SAFETY: a test fixture.
    unsafe impl Sync for TestableDiagJob {}

    impl TestableDiagJob {
        pub(crate) const fn new(
            implemented_request: Option<&[u8]>,
            request_length: u8,
            prefix_length: u8,
        ) -> Self {
            Self {
                base: JobBase::new(
                    implemented_request,
                    request_length,
                    prefix_length,
                    DiagSessionMask::ALL_SESSIONS,
                ),
                authenticated: Cell::new(true),
            }
        }

        pub(crate) const fn with_lengths(
            implemented_request: Option<&[u8]>,
            request_length: u8,
            prefix_length: u8,
            request_payload_length: u8,
            response_length: u8,
            sessions: DiagSessionMask,
        ) -> Self {
            Self {
                base: JobBase::with_lengths(
                    implemented_request,
                    request_length,
                    prefix_length,
                    request_payload_length,
                    response_length,
                    sessions,
                ),
                authenticated: Cell::new(true),
            }
        }
    }

    impl DiagAuthenticator for TestableDiagJob {
        fn is_authenticated(&self, _address: u16) -> bool {
            self.authenticated.get()
        }

        fn not_authenticated_return_code(&self) -> DiagReturnCode {
            DiagReturnCode::IsoSecurityAccessDenied
        }
    }

    impl DiagJob for TestableDiagJob {
        fn base(&self) -> &JobBase {
            &self.base
        }

        fn verify(&self, request: Request<'_>) -> DiagReturnCode {
            if request.is_empty() { DiagReturnCode::IsoInvalidFormat } else { DiagReturnCode::Ok }
        }

        fn authenticator(&self) -> &dyn DiagAuthenticator {
            self
        }
    }

    fn job(bytes: &'static [u8]) -> &'static TestableDiagJob {
        Box::leak(Box::new(TestableDiagJob::new(Some(bytes), bytes.len() as u8, 0)))
    }

    fn root() -> &'static DiagJobRoot {
        Box::leak(Box::new(DiagJobRoot::new()))
    }

    #[test]
    fn add_one_job_identical_jobs_and_different_jobs() {
        let _guard = serial();
        let root = root();
        let job1 = job(&[0x22]);
        assert_eq!(root.base().add_abstract_diag_job(job1), JobErrorCode::JobAdded);
        assert_eq!(root.base().add_abstract_diag_job(job1), JobErrorCode::JobNotAdded);
        let job2 = job(&[0x22]);
        assert_eq!(root.base().add_abstract_diag_job(job2), JobErrorCode::JobNotAdded);
        let job3 = job(&[0x2E]);
        assert_eq!(root.base().add_abstract_diag_job(job3), JobErrorCode::JobAdded);
        // The root cannot be added to itself.
        assert_eq!(root.base().add_abstract_diag_job(root), JobErrorCode::JobNotAdded);
        // Two instances with the same content are one job.
        let a = job(&[0x22]);
        let b = job(&[0x22]);
        assert_eq!(a.base().add_abstract_diag_job(b), JobErrorCode::JobNotAdded);
    }

    #[test]
    fn add_returns_job_not_added_if_job_was_added_earlier() {
        let _guard = serial();
        let root = root();
        let jobs = [job(&[0x22]), job(&[0x2E]), job(&[0x33]), job(&[0x33])];
        assert_eq!(root.base().add_abstract_diag_job(jobs[0]), JobErrorCode::JobAdded);
        assert_eq!(root.base().add_abstract_diag_job(jobs[1]), JobErrorCode::JobAdded);
        assert_eq!(root.base().add_abstract_diag_job(jobs[2]), JobErrorCode::JobAdded);
        for job in jobs {
            assert_eq!(root.base().add_abstract_diag_job(job), JobErrorCode::JobNotAdded);
        }
    }

    #[test]
    fn add_returns_job_not_added_if_job_is_not_family() {
        let _guard = serial();
        let request: &'static [u8] = &[0x22, 0x01, 0xA0];
        let job1 = Box::leak(Box::new(TestableDiagJob::new(Some(request), 3, 0)));
        let job2 = Box::leak(Box::new(TestableDiagJob::new(Some(request), 3, 1)));
        assert_eq!(job1.base().add_abstract_diag_job(job2), JobErrorCode::JobNotAdded);
        // Family, but no first child to take it.
        let root = root();
        assert_eq!(root.base().add_abstract_diag_job(job2), JobErrorCode::JobNotAdded);
        // Family, but the first child cannot add it either.
        let job3 = Box::leak(Box::new(TestableDiagJob::new(Some(request), 3, 2)));
        assert_eq!(root.base().add_abstract_diag_job(job1), JobErrorCode::JobAdded);
        assert_eq!(root.base().add_abstract_diag_job(job3), JobErrorCode::JobNotAdded);
    }

    #[test]
    fn subfunctions_hang_below_their_service() {
        let _guard = serial();
        let root = root();
        let service = job(&[0x31]);
        let start: &'static Subfunction =
            Box::leak(Box::new(Subfunction::new(&[0x31, 0x01], DiagSessionMask::ALL_SESSIONS)));
        let stop: &'static Subfunction =
            Box::leak(Box::new(Subfunction::new(&[0x31, 0x02], DiagSessionMask::ALL_SESSIONS)));
        assert_eq!(root.base().add_abstract_diag_job(service), JobErrorCode::JobAdded);
        assert_eq!(root.base().add_abstract_diag_job(start), JobErrorCode::JobAdded);
        assert_eq!(root.base().add_abstract_diag_job(stop), JobErrorCode::JobAdded);
        assert!(core::ptr::eq(service.base().first_child().unwrap().base(), start.base()));
        assert!(core::ptr::eq(start.base().next_job().unwrap().base(), stop.base()));
        root.base().remove_abstract_diag_job(start);
        assert!(core::ptr::eq(service.base().first_child().unwrap().base(), stop.base()));
        root.base().remove_abstract_diag_job(stop);
        assert!(service.base().first_child().is_none());
    }

    #[test]
    fn remove_jobs_in_different_order_works() {
        let _guard = serial();
        let root = root();
        let jobs = [job(&[0x22]), job(&[0x2E]), job(&[0x31]), job(&[0x3F]), job(&[0x10])];
        for job in jobs {
            root.base().add_abstract_diag_job(job);
        }
        root.base().remove_abstract_diag_job(root);
        root.base().remove_abstract_diag_job(jobs[0]);
        root.base().remove_abstract_diag_job(jobs[2]);
        root.base().remove_abstract_diag_job(jobs[4]);
        assert!(core::ptr::eq(root.base().first_child().unwrap().base(), jobs[1].base()));
        assert!(core::ptr::eq(jobs[1].base().next_job().unwrap().base(), jobs[3].base()));
        assert!(jobs[3].base().next_job().is_none());
    }

    #[test]
    fn defaults_next_job_family_and_compare() {
        let _guard = serial();
        let job1 = job(&[0x22]);
        assert_eq!(job1.base().default_return_code(), DiagReturnCode::IsoGeneralReject);
        assert_eq!(job1.base().request_id(), 0x22);
        job1.base().set_next_job(Some(job1));
        assert!(job1.base().next_job().is_none());
        assert!(!job1.base().is_family(Some(&[0x22])));
        assert!(compare(None, None, 1));
        assert!(!compare(None, Some(&[0x22]), 1));
        assert!(!compare(Some(&[0x22]), None, 1));
        assert!(compare(Some(&[0x22]), Some(&[0x22, 0x33]), 1));
        let root = root();
        assert_eq!(root.base().request_id(), 0);
        assert!(root.base().implemented_request().is_none());
        assert_eq!(root.base().request_length(), 0);
        job1.base().set_enable_suppress_positive_response(root.base());
    }

    #[test]
    #[should_panic(expected = "requested length must be greater than the prefix length")]
    fn constructor_panics_if_prefix_length_is_not_smaller_than_request_length() {
        let _ = JobBase::new(Some(&[0x22]), 1, 1, DiagSessionMask::ALL_SESSIONS);
    }

    #[test]
    fn constructor_does_no_prefix_length_check_if_request_length_is_zero() {
        let _ = JobBase::with_lengths(None, 0, 0, 0, 1, DiagSessionMask::ALL_SESSIONS);
        let _ = JobBase::with_lengths(Some(&[0x22]), 1, 0, 0, 1, DiagSessionMask::ALL_SESSIONS);
    }

    #[test]
    fn execute_returns_request_out_of_range_if_session_does_not_match() {
        let _guard = serial();
        set_default_diag_session_manager(Some(&SESSION_MANAGER));
        SESSION_MANAGER.reset(&APPLICATION_EXTENDED_SESSION);
        let request = message(6);
        let connection = new_connection(request);
        let job: &'static TestableDiagJob = Box::leak(Box::new(TestableDiagJob::with_lengths(
            Some(&[0x22]),
            0,
            0,
            VARIABLE_REQUEST_LENGTH,
            VARIABLE_RESPONSE_LENGTH,
            DiagSessionMask::EMPTY,
        )));
        assert_eq!(
            job.execute(connection, cells(&[0x22, 0x01, 0x00])),
            DiagReturnCode::IsoRequestOutOfRange
        );
        // With request length zero, acceptedJob is not called.
        assert!(SESSION_MANAGER.accepted_ids().is_empty());
    }

    #[test]
    fn execute_returns_security_access_denied_if_authentication_fails() {
        let _guard = serial();
        set_default_diag_session_manager(Some(&SESSION_MANAGER));
        SESSION_MANAGER.reset(&APPLICATION_EXTENDED_SESSION);
        let connection = new_connection(message(6));
        let request = cells(&[0x22, 0x01, 0x00]);
        let short: &'static TestableDiagJob =
            Box::leak(Box::new(TestableDiagJob::new(Some(&[0x22]), 0, 0)));
        short.authenticated.set(false);
        assert_eq!(short.execute(connection, request), DiagReturnCode::IsoSecurityAccessDenied);
        assert!(SESSION_MANAGER.accepted_ids().is_empty());
        let valid: &'static TestableDiagJob =
            Box::leak(Box::new(TestableDiagJob::new(Some(&[0x22]), 1, 0)));
        valid.authenticated.set(false);
        assert_eq!(valid.execute(connection, request), DiagReturnCode::IsoSecurityAccessDenied);
        assert_eq!(SESSION_MANAGER.accepted_ids(), [0x22]);
        assert_eq!(
            SESSION_MANAGER.accepted.lock().unwrap_or_else(|poisoned| poisoned.into_inner())[0].1,
            [0x22, 0x01, 0x00]
        );
    }

    #[test]
    fn execute_checks_the_payload_and_response_lengths() {
        let _guard = serial();
        set_default_diag_session_manager(Some(&SESSION_MANAGER));
        SESSION_MANAGER.reset(&APPLICATION_EXTENDED_SESSION);
        let connection = new_connection(message(6));
        let request = cells(&[0x22, 0x01, 0x00]);
        // A fixed payload length of 0 does not match the two bytes that follow.
        let extended: &'static TestableDiagJob = Box::leak(Box::new(
            TestableDiagJob::with_lengths(Some(&[0x22]), 1, 0, 0, 1, DiagSessionMask::ALL_SESSIONS),
        ));
        assert_eq!(extended.execute(connection, request), DiagReturnCode::IsoInvalidFormat);
        assert_eq!(SESSION_MANAGER.accepted_ids(), [0x22]);
        // With request length 0 the format error does not reach acceptedJob.
        SESSION_MANAGER.reset(&APPLICATION_EXTENDED_SESSION);
        let two: &'static TestableDiagJob = Box::leak(Box::new(TestableDiagJob::with_lengths(
            Some(&[0x22]),
            0,
            0,
            2,
            1,
            DiagSessionMask::ALL_SESSIONS,
        )));
        assert_eq!(two.execute(connection, request), DiagReturnCode::IsoInvalidFormat);
        assert!(SESSION_MANAGER.accepted_ids().is_empty());
        // A response of 7 bytes does not fit a 6-byte message with two identifiers.
        let long: &'static TestableDiagJob = Box::leak(Box::new(TestableDiagJob::with_lengths(
            Some(&[0x22, 0x0B]),
            2,
            1,
            2,
            7,
            DiagSessionMask::ALL_SESSIONS,
        )));
        let connection = new_connection(message(6));
        assert_eq!(long.execute(connection, request), DiagReturnCode::IsoResponseTooLong);
        assert_eq!(SESSION_MANAGER.accepted_ids(), [0x220B]);
    }

    #[test]
    fn execute_returns_general_reject_if_everything_works_but_process_fails() {
        let _guard = serial();
        set_default_diag_session_manager(Some(&SESSION_MANAGER));
        SESSION_MANAGER.reset(&APPLICATION_EXTENDED_SESSION);
        let job: &'static TestableDiagJob = Box::leak(Box::new(TestableDiagJob::with_lengths(
            Some(&[0x22, 0x0B]),
            2,
            1,
            2,
            1,
            DiagSessionMask::ALL_SESSIONS,
        )));
        job.base().enable_suppress_positive_response(true);
        let connection = new_connection(message(6));
        let request1 = cells(&[0x22, 0x80, 0x01]);
        assert_eq!(job.execute(connection, request1), DiagReturnCode::IsoGeneralReject);
        // The suppress bit was cleared in the request and noted on the connection.
        assert_eq!(request1[1].get(), 0x00);
        let connection = new_connection(message(6));
        let request2 = cells(&[0x22, 0x00, 0x01]);
        assert_eq!(job.execute(connection, request2), DiagReturnCode::IsoGeneralReject);
        // Both calls accepted the job with what followed its own byte (the second of its
        // two-byte request; the first is the parent's prefix).
        assert_eq!(SESSION_MANAGER.accepted_ids(), [0x220B, 0x220B]);
        assert_eq!(
            SESSION_MANAGER.accepted.lock().unwrap_or_else(|poisoned| poisoned.into_inner())[1].1,
            [0x00, 0x01]
        );
    }

    #[test]
    fn execute_asks_the_children_and_answers_with_the_default_when_none_is_responsible() {
        let _guard = serial();
        set_default_diag_session_manager(Some(&SESSION_MANAGER));
        SESSION_MANAGER.reset(&APPLICATION_DEFAULT_SESSION);
        let root = root();
        let service: &'static Service = Box::leak(Box::new(
            Service::new(0x22, DiagSessionMask::ALL_SESSIONS)
                .with_default_return_code(DiagReturnCode::IsoRequestOutOfRange),
        ));
        root.base().add_abstract_diag_job(service);
        let connection = new_connection(message(8));
        connection.set_addresses(0x10, 0x2A, 0x22);
        assert_eq!(
            root.execute(connection, cells(&[0x22, 0xF1, 0x90])),
            DiagReturnCode::IsoRequestOutOfRange
        );
        assert_eq!(SESSION_MANAGER.accepted_ids(), [0x22]);
        assert_eq!(connection.num_identifiers(), 1);
        // An unknown service: the root's default.
        let connection = new_connection(message(8));
        connection.set_addresses(0x10, 0x2A, 0x11);
        assert_eq!(
            root.execute(connection, cells(&[0x11, 0x01])),
            DiagReturnCode::IsoServiceNotSupported
        );
    }

    // DiagJobRootTest.
    #[test]
    fn root_verify_and_functional_tester_present() {
        let _guard = serial();
        set_default_diag_session_manager(Some(&SESSION_MANAGER));
        SESSION_MANAGER.reset(&APPLICATION_DEFAULT_SESSION);
        let root = root();
        assert_eq!(root.verify(cells(&[])), DiagReturnCode::IsoGeneralReject);
        assert_eq!(root.verify(cells(&[0x22, 0x00, 0x00])), DiagReturnCode::Ok);
        // No session timeout runs, so a functional TesterPresent that suppresses its answer
        // is ignored.
        let connection = new_connection(message(8));
        connection.set_addresses(0x10, 0xDF, 0x3E);
        connection.set_addresses(0x10, 0xDF, 0x3E);
        assert_eq!(root.execute(connection, cells(&[0x3E, 0x80])), DiagReturnCode::Ok);
        // With an active timeout, the tree is asked (no job: service not supported).
        SESSION_MANAGER.timeout_active.set(true);
        assert_eq!(
            root.execute(connection, cells(&[0x3E, 0x80])),
            DiagReturnCode::IsoServiceNotSupported
        );
        SESSION_MANAGER.timeout_active.set(false);
        // Another service, a short TesterPresent, or one that wants its answer.
        connection.set_addresses(0x10, 0xDF, 0x21);
        assert_eq!(
            root.execute(connection, cells(&[0x21, 0x80])),
            DiagReturnCode::IsoServiceNotSupported
        );
        connection.set_addresses(0x10, 0xDF, 0x3E);
        assert_eq!(
            root.execute(connection, cells(&[0x3E])),
            DiagReturnCode::IsoServiceNotSupported
        );
        assert_eq!(
            root.execute(connection, cells(&[0x3E, 0x00])),
            DiagReturnCode::IsoServiceNotSupported
        );
        // An incoming negative response is never answered.
        connection.set_addresses(0x10, 0x2A, 0x7F);
        assert_eq!(root.execute(connection, cells(&[0x7F, 0x22, 0x11])), DiagReturnCode::Ok);
    }

    // ServiceTest.
    #[test]
    fn service_constructors_and_verify() {
        let _guard = serial();
        set_default_diag_session_manager(Some(&SESSION_MANAGER));
        SESSION_MANAGER.reset(&APPLICATION_DEFAULT_SESSION);
        let service = Service::new(0x01, DiagSessionMask::APPLICATION_DEFAULT_SESSION_MASK);
        assert_eq!(service.base().request_id(), 0x01);
        let fixed =
            Service::with_lengths(0x01, 3, 3, DiagSessionMask::APPLICATION_DEFAULT_SESSION_MASK);
        assert_eq!(fixed.base().request_id(), 0x01);
        assert_eq!(service.verify(cells(&[0x01, 0x02, 0x03])), DiagReturnCode::Ok);
        assert_eq!(service.verify(cells(&[0x02, 0x02, 0x03])), DiagReturnCode::NotResponsible);
        SESSION_MANAGER.session.set(&APPLICATION_EXTENDED_SESSION);
        assert_eq!(
            service.verify(cells(&[0x01, 0x02, 0x03])),
            DiagReturnCode::IsoServiceNotSupportedInActiveSession
        );
    }

    // SubfunctionTest.
    #[test]
    fn subfunction_constructors_and_verify() {
        let _guard = serial();
        set_default_diag_session_manager(Some(&SESSION_MANAGER));
        SESSION_MANAGER.reset(&APPLICATION_DEFAULT_SESSION);
        let subfunction = Subfunction::new(&[0x22, 0x00], DiagSessionMask::ALL_SESSIONS);
        assert_eq!(subfunction.base().request_id(), 0x2200);
        assert_eq!(subfunction.base().implemented_request(), Some(&[0x22, 0x00][..]));
        assert_eq!(subfunction.base().request_length(), 2);
        let fixed = Subfunction::with_lengths(&[0x31, 0x00], 1, 2, DiagSessionMask::ALL_SESSIONS);
        assert_eq!(fixed.base().request_id(), 0x3100);
        assert_eq!(subfunction.verify(cells(&[])), DiagReturnCode::IsoInvalidFormat);
        assert_eq!(subfunction.verify(cells(&[0x02, 0x01])), DiagReturnCode::NotResponsible);
        let extended_only =
            Subfunction::new(&[0x22, 0x00], DiagSessionMask::APPLICATION_EXTENDED_SESSION_MASK);
        assert_eq!(
            extended_only.verify(cells(&[0x00, 0x01])),
            DiagReturnCode::IsoSubfunctionNotSupportedInActiveSession
        );
        assert_eq!(subfunction.verify(cells(&[0x00, 0x01])), DiagReturnCode::Ok);
    }
}
