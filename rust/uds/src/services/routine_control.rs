// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Service 0x31 and its subfunctions: the port of `services/routinecontrol/RoutineControl.h`,
//! `StartRoutine.h`, `StopRoutine.h` and `RequestRoutineResults.h`.

use crate::codes::{DiagReturnCode, ServiceId};
use crate::job::{DiagJob, JobBase, Request, Service, Subfunction};
use crate::session::DiagSessionMask;

/// RoutineControl: at least four bytes, answered by a subfunction or
/// `subFunctionNotSupported`.
pub struct RoutineControl {
    service: Service,
}

impl RoutineControl {
    /// startRoutine.
    pub const START_ROUTINE: u8 = 0x01;
    /// stopRoutine.
    pub const STOP_ROUTINE: u8 = 0x02;
    /// requestRoutineResults.
    pub const REQUEST_ROUTINE_RESULTS: u8 = 0x03;

    /// The service.
    pub const fn new() -> Self {
        Self {
            service: Service::new(ServiceId::ROUTINE_CONTROL, DiagSessionMask::ALL_SESSIONS)
                .with_default_return_code(DiagReturnCode::IsoSubfunctionNotSupported)
                .with_suppress_positive_response(),
        }
    }
}

impl Default for RoutineControl {
    fn default() -> Self {
        Self::new()
    }
}

impl DiagJob for RoutineControl {
    fn base(&self) -> &JobBase {
        self.service.base()
    }

    fn verify(&self, request: Request<'_>) -> DiagReturnCode {
        let result = self.service.verify(request);
        if result == DiagReturnCode::Ok && request.len() < 4 {
            return DiagReturnCode::IsoInvalidFormat;
        }
        result
    }
}

macro_rules! routine_subfunction {
    ($name:ident, $subfunction:expr, $doc:literal) => {
        #[doc = $doc]
        pub struct $name {
            subfunction: Subfunction,
        }

        impl $name {
            /// The subfunction; routines hang below it, else `requestOutOfRange`.
            pub const fn new() -> Self {
                Self {
                    subfunction: Subfunction::new(
                        &[ServiceId::ROUTINE_CONTROL, $subfunction],
                        DiagSessionMask::ALL_SESSIONS,
                    )
                    .with_default_return_code(DiagReturnCode::IsoRequestOutOfRange),
                }
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl DiagJob for $name {
            fn base(&self) -> &JobBase {
                self.subfunction.base()
            }

            fn verify(&self, request: Request<'_>) -> DiagReturnCode {
                self.subfunction.verify(request)
            }
        }
    };
}

routine_subfunction!(
    StartRoutine,
    RoutineControl::START_ROUTINE,
    "RoutineControl startRoutine (`31 01`)."
);
routine_subfunction!(
    StopRoutine,
    RoutineControl::STOP_ROUTINE,
    "RoutineControl stopRoutine (`31 02`)."
);
routine_subfunction!(
    RequestRoutineResults,
    RoutineControl::REQUEST_ROUTINE_RESULTS,
    "RoutineControl requestRoutineResults (`31 03`)."
);
