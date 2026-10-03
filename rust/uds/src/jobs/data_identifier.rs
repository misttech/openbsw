// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Jobs below ReadDataByIdentifier and WriteDataByIdentifier: the port of
//! `jobs/DataIdentifierJob.h` and `jobs/ReadIdentifierFromMemory.h`.

use crate::codes::DiagReturnCode;
use crate::connection::IncomingDiagConnection;
use crate::job::{DiagJob, JobBase, Request, compare, request_byte};
use crate::session::DiagSessionMask;

/// A job for one two-byte data identifier below a service (`DataIdentifierJob`).
pub struct DataIdentifierJob {
    base: JobBase,
}

impl DataIdentifierJob {
    /// A job for `implemented_request` (service, id high, id low) in `allowed_sessions`.
    pub const fn new(implemented_request: &[u8; 3], allowed_sessions: DiagSessionMask) -> Self {
        Self { base: JobBase::new(Some(implemented_request), 3, 1, allowed_sessions) }
    }

    /// A job with fixed request payload and response lengths.
    pub const fn with_lengths(
        implemented_request: &[u8; 3],
        request_payload_length: u8,
        response_length: u8,
        allowed_sessions: DiagSessionMask,
    ) -> Self {
        Self {
            base: JobBase::with_lengths(
                Some(implemented_request),
                3,
                1,
                request_payload_length,
                response_length,
                allowed_sessions,
            ),
        }
    }

    /// The base.
    pub fn base(&self) -> &JobBase {
        &self.base
    }

    /// `DataIdentifierJob::verify`: the two id bytes must match.
    pub fn verify(&self, request: Request<'_>) -> DiagReturnCode {
        let implemented = self.base.implemented_request().unwrap_or(&[]);
        if request.len() < 2 {
            return DiagReturnCode::NotResponsible;
        }
        let id = [request_byte(request, 0), request_byte(request, 1)];
        if !compare(Some(&id), implemented.get(1..), 2) {
            return DiagReturnCode::NotResponsible;
        }
        DiagReturnCode::Ok
    }
}

impl DiagJob for DataIdentifierJob {
    fn base(&self) -> &JobBase {
        &self.base
    }

    fn verify(&self, request: Request<'_>) -> DiagReturnCode {
        DataIdentifierJob::verify(self, request)
    }
}

/// Answers a ReadDataByIdentifier of one identifier with fixed bytes
/// (`ReadIdentifierFromMemory`).
pub struct ReadIdentifierFromMemory {
    job: DataIdentifierJob,
    response_data: &'static [u8],
}

impl ReadIdentifierFromMemory {
    /// A job answering `identifier` with `response_data` in `allowed_sessions`.
    pub const fn new(
        identifier: u16,
        response_data: &'static [u8],
        allowed_sessions: DiagSessionMask,
    ) -> Self {
        let [high, low] = identifier.to_be_bytes();
        Self {
            job: DataIdentifierJob::new(
                &[crate::codes::ServiceId::READ_DATA_BY_IDENTIFIER, high, low],
                allowed_sessions,
            ),
            response_data,
        }
    }
}

impl DiagJob for ReadIdentifierFromMemory {
    fn base(&self) -> &JobBase {
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
        let _ = response.append_data(self.response_data);
        let _ = connection.send_positive_response_internal(response.length() as u16, self);
        DiagReturnCode::Ok
    }
}

// Ported from DataIdentifierJobTest.cpp and ReadIdentifierFromMemoryJobTest.cpp.
#[cfg(test)]
mod tests {
    extern crate std;

    use std::boxed::Box;

    use super::*;
    use crate::job::tests::{SESSION_MANAGER, cells, message, new_connection, serial};
    use crate::job::{EMPTY_RESPONSE, set_default_diag_session_manager};
    use crate::session::APPLICATION_DEFAULT_SESSION;

    #[test]
    fn verify_jobs_responsible_for_their_data_identifier() {
        let read = DataIdentifierJob::new(&[0x22, 0x25, 0x09], DiagSessionMask::ALL_SESSIONS);
        let write = DataIdentifierJob::with_lengths(
            &[0x2E, 0x17, 0x2A],
            13,
            EMPTY_RESPONSE,
            DiagSessionMask::ALL_SESSIONS,
        );
        assert_eq!(read.verify(cells(&[0x25, 0x09])), DiagReturnCode::Ok);
        assert_eq!(write.verify(cells(&[0x17, 0x2A])), DiagReturnCode::Ok);
        assert_eq!(read.verify(cells(&[0x25, 0x08])), DiagReturnCode::NotResponsible);
        assert_eq!(write.verify(cells(&[0x18, 0x2A])), DiagReturnCode::NotResponsible);
        assert_eq!(read.base().request_id(), 0x222509);
    }

    #[test]
    fn read_identifier_from_memory_answers_with_its_bytes() {
        let _guard = serial();
        set_default_diag_session_manager(Some(&SESSION_MANAGER));
        SESSION_MANAGER.reset(&APPLICATION_DEFAULT_SESSION);
        static TESTDATA: [u8; 4] = [0x00, 0x01, 0x02, 0x03];
        let cut: &'static ReadIdentifierFromMemory =
            Box::leak(Box::new(ReadIdentifierFromMemory::new(
                0x4242,
                &TESTDATA,
                DiagSessionMask::APPLICATION_DEFAULT_SESSION_MASK,
            )));
        let request = message(10);
        request.set_source_address(0xF1);
        request.set_target_address(0x10);
        request.append(&[0x22, 0x42, 0x42]);
        request.set_payload_length(3);
        let connection = new_connection(request);
        // The service claimed the first byte as an identifier already.
        connection.add_identifier();
        assert_eq!(cut.execute(connection, cells(&[0x42, 0x42])), DiagReturnCode::Ok);
        // The answer follows the identifiers (22 42 42) in the request's own buffer.
        let bytes: std::vec::Vec<u8> =
            request.buffer()[3..7].iter().map(core::cell::Cell::get).collect();
        assert_eq!(bytes, TESTDATA);
        assert_eq!(SESSION_MANAGER.accepted_ids(), [0x224242]);
    }
}
