// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Port of `libs/bsw/uds`, the ISO 14229 diagnostic layer, as the Zephyr demo is built with
//! it (the OpenBSW commit the demo pins, where the dispatcher is `DiagDispatcher2`).
//!
//! - [`codes`]: the service ids, the negative response codes ([`DiagReturnCode`]) and the
//!   message constants.
//! - [`session`]: the diagnostic sessions, their masks and the [`DiagSessionManager`]
//!   interface.
//! - [`authenticator`]: who may send a request.
//! - [`job`]: the tree of diagnostic jobs: [`DiagJob`], its [`JobBase`], the root, services,
//!   subfunctions and data identifier jobs.
//! - [`response`]: [`PositiveResponse`], the bytes a job appends to its answer.
//! - [`connection`]: [`IncomingDiagConnection`], one request from a tester and its answer.
//! - [`config`] and [`dispatcher`]: the connections and the send queue, the
//!   [`DiagConnectionManager`] and the [`DiagDispatcher`], the transport layer the router
//!   hands diagnostic requests to.
//! - [`services`] and [`jobs`]: the services the demo registers.
//! - [`UDS`]: the logger component.
//!
//! Requests are slices of the transport message's cells (`&[Cell<u8>]`), since the C++
//! clears the suppress-positive-response bit in the request buffer as it goes.
//! Not ported: outgoing diagnostic connections, nested requests, the async job helpers,
//! the authentication variants of the base classes, and the services the demo does not
//! register. The connection manager keeps the count of outgoing connections the C++
//! configuration declares, for the shutdown log line.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

pub mod authenticator;
pub mod codes;
pub mod config;
pub mod connection;
pub mod dispatcher;
pub mod job;
pub mod jobs;
pub mod lifecycle;
pub mod persistence;
pub mod response;
pub mod services;
pub mod session;

use openbsw_util::logger::LoggerComponent;

pub use authenticator::{DEFAULT_DIAG_AUTHENTICATOR, DefaultDiagAuthenticator, DiagAuthenticator};
pub use codes::{ConnectionErrorCode, DiagCodes, DiagReturnCode, ServiceId};
pub use config::{DiagnosisConfiguration, TransportConfiguration};
pub use connection::IncomingDiagConnection;
pub use dispatcher::{DiagConnectionManager, DiagDispatcher};
pub use job::{
    DiagJob, DiagJobRoot, JobBase, JobErrorCode, Request, ResponseSendResult, Service, Subfunction,
    request_byte, set_default_diag_session_manager, set_diag_job_root,
};
pub use lifecycle::{ShutdownType, UdsLifecycleConnector};
pub use persistence::SessionPersistence;
pub use response::PositiveResponse;
pub use session::{
    APPLICATION_DEFAULT_SESSION, APPLICATION_EXTENDED_SESSION, DiagSession,
    DiagSessionChangedListener, DiagSessionManager, DiagSessionMask, PROGRAMMING_SESSION,
    SessionListenerNode, SessionType,
};

/// The `UDS` logger component (`UdsLogger.h`).
pub static UDS: LoggerComponent = LoggerComponent::new();

/// The application's `UdsConfig.h` values the library reads, as the demo defines them.
pub mod uds_config {
    /// How long a non-default session lives without a TesterPresent
    /// (`UdsVmsConstants::TESTER_PRESENT_TIMEOUT_MS`).
    pub const TESTER_PRESENT_TIMEOUT_MS: u32 = 5000;
    /// Spare bytes behind the dispatcher's busy message
    /// (`UdsVmsConstants::BUSY_MESSAGE_EXTRA_BYTES`).
    pub const BUSY_MESSAGE_EXTRA_BYTES: usize = 7;
}
