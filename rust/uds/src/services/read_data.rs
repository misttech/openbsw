// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Service 0x22: the port of `services/readdata/ReadDataByIdentifier.h`. The data
//! identifier jobs hang below it.

use crate::codes::{DiagReturnCode, ServiceId};
use crate::job::{DiagJob, JobBase, Request, Service};
use crate::session::DiagSessionMask;

/// ReadDataByIdentifier: a three-byte request, answered by a child or
/// `requestOutOfRange`.
pub struct ReadDataByIdentifier {
    service: Service,
}

impl ReadDataByIdentifier {
    const EXPECTED_REQUEST_LENGTH: usize = 3;

    /// The service.
    pub const fn new() -> Self {
        Self {
            service: Service::new(
                ServiceId::READ_DATA_BY_IDENTIFIER,
                DiagSessionMask::ALL_SESSIONS,
            )
            .with_default_return_code(DiagReturnCode::IsoRequestOutOfRange),
        }
    }
}

impl Default for ReadDataByIdentifier {
    fn default() -> Self {
        Self::new()
    }
}

impl DiagJob for ReadDataByIdentifier {
    fn base(&self) -> &JobBase {
        self.service.base()
    }

    fn verify(&self, request: Request<'_>) -> DiagReturnCode {
        let result = self.service.verify(request);
        if result == DiagReturnCode::Ok && request.len() != Self::EXPECTED_REQUEST_LENGTH {
            return DiagReturnCode::IsoInvalidFormat;
        }
        result
    }
}

// Ported from ReadDataByIdentifierTest.cpp.
#[cfg(test)]
mod tests {
    extern crate std;

    use std::boxed::Box;

    use super::*;
    use crate::job::set_default_diag_session_manager;
    use crate::job::tests::{SESSION_MANAGER, cells, message, new_connection, serial};
    use crate::session::APPLICATION_DEFAULT_SESSION;

    #[test]
    fn default_return_code_and_verify() {
        let _guard = serial();
        set_default_diag_session_manager(Some(&SESSION_MANAGER));
        SESSION_MANAGER.reset(&APPLICATION_DEFAULT_SESSION);
        let cut: &'static ReadDataByIdentifier = Box::leak(Box::new(ReadDataByIdentifier::new()));
        assert_eq!(cut.base().default_return_code(), DiagReturnCode::IsoRequestOutOfRange);
        let connection = new_connection(message(8));
        // A valid request with no child answers the default.
        assert_eq!(
            cut.execute(connection, cells(&[0x22, 0x00, 0x01])),
            DiagReturnCode::IsoRequestOutOfRange
        );
        assert_eq!(cut.execute(connection, cells(&[0x22, 0x00])), DiagReturnCode::IsoInvalidFormat);
        assert_eq!(
            cut.execute(connection, cells(&[0x31, 0x00, 0x01])),
            DiagReturnCode::NotResponsible
        );
    }
}
