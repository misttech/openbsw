// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Service 0x2E: the port of `services/writedata/WriteDataByIdentifier.h`.

use crate::codes::{DiagReturnCode, ServiceId};
use crate::job::{DiagJob, JobBase, Request, Service};
use crate::session::DiagSessionMask;

/// WriteDataByIdentifier: at least three bytes, answered by a child or
/// `requestOutOfRange`.
pub struct WriteDataByIdentifier {
    service: Service,
}

impl WriteDataByIdentifier {
    /// The service.
    pub const fn new() -> Self {
        Self {
            service: Service::new(
                ServiceId::WRITE_DATA_BY_IDENTIFIER,
                DiagSessionMask::ALL_SESSIONS,
            )
            .with_default_return_code(DiagReturnCode::IsoRequestOutOfRange),
        }
    }
}

impl Default for WriteDataByIdentifier {
    fn default() -> Self {
        Self::new()
    }
}

impl DiagJob for WriteDataByIdentifier {
    fn base(&self) -> &JobBase {
        self.service.base()
    }

    fn verify(&self, request: Request<'_>) -> DiagReturnCode {
        let result = self.service.verify(request);
        if result == DiagReturnCode::Ok && request.len() < 3 {
            return DiagReturnCode::IsoInvalidFormat;
        }
        result
    }
}
