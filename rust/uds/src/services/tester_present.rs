// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Service 0x3E: the port of `services/testerpresent/TesterPresent.h`.

use crate::codes::{DiagReturnCode, ServiceId};
use crate::connection::IncomingDiagConnection;
use crate::job::{DiagJob, JobBase, Request, Service, request_byte};
use crate::session::DiagSessionMask;

/// Answers `3E 00` with `7E 00`, honoring the suppress-positive-response bit.
pub struct TesterPresent {
    service: Service,
}

impl TesterPresent {
    const TESTER_PRESENT_ANSWER: u8 = 0x00;
    const EXPECTED_REQUEST_LENGTH: u8 = 1;
    const RESPONSE_LENGTH: u8 = 1;

    /// The service.
    pub const fn new() -> Self {
        Self {
            service: Service::with_lengths(
                ServiceId::TESTER_PRESENT,
                Self::EXPECTED_REQUEST_LENGTH,
                Self::RESPONSE_LENGTH,
                DiagSessionMask::ALL_SESSIONS,
            )
            .with_suppress_positive_response(),
        }
    }
}

impl Default for TesterPresent {
    fn default() -> Self {
        Self::new()
    }
}

impl DiagJob for TesterPresent {
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
        if request_byte(request, 0) == Self::TESTER_PRESENT_ANSWER {
            let response = connection.release_request_get_response();
            let _ = response.append_u8(Self::TESTER_PRESENT_ANSWER);
            let _ = connection.send_positive_response_internal(response.length() as u16, self);
            DiagReturnCode::Ok
        } else {
            DiagReturnCode::IsoSubfunctionNotSupported
        }
    }
}

// Ported from TesterPresentTest.cpp.
#[cfg(test)]
mod tests {
    extern crate std;

    use std::boxed::Box;

    use super::*;
    use crate::job::set_default_diag_session_manager;
    use crate::job::tests::{SESSION_MANAGER, cells, message, new_connection, serial};
    use crate::session::APPLICATION_DEFAULT_SESSION;

    #[test]
    fn process_called_by_execute_returns_ok_or_subfunction_not_supported() {
        let _guard = serial();
        set_default_diag_session_manager(Some(&SESSION_MANAGER));
        SESSION_MANAGER.reset(&APPLICATION_DEFAULT_SESSION);
        let cut: &'static TesterPresent = Box::leak(Box::new(TesterPresent::new()));
        let request = message(8);
        request.append(&[0x3E, 0x00]);
        request.set_payload_length(2);
        let connection = new_connection(request);
        assert_eq!(cut.execute(connection, cells(&[0x3E, 0x00])), DiagReturnCode::Ok);
        let request = message(8);
        request.append(&[0x3E, 0x01]);
        request.set_payload_length(2);
        let connection = new_connection(request);
        assert_eq!(
            cut.execute(connection, cells(&[0x3E, 0x01])),
            DiagReturnCode::IsoSubfunctionNotSupported
        );
    }
}
