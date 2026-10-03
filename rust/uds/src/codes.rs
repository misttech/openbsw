// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The constants of `UdsConstants.h`, `DiagReturnCode.h`, `DiagCodes.h` and
//! `connection/ErrorCode.h`.

/// The service identifiers (`ServiceId`).
pub struct ServiceId;

impl ServiceId {
    /// DiagnosticSessionControl.
    pub const DIAGNOSTIC_SESSION_CONTROL: u8 = 0x10;
    /// ECUReset.
    pub const ECU_RESET: u8 = 0x11;
    /// ClearDiagnosticInformation.
    pub const CLEAR_DIAGNOSTIC_INFORMATION: u8 = 0x14;
    /// ReadDTCInformation.
    pub const READ_DTC_INFORMATION: u8 = 0x19;
    /// ReadDataByIdentifier.
    pub const READ_DATA_BY_IDENTIFIER: u8 = 0x22;
    /// ReadMemoryByAddress.
    pub const READ_MEMORY_BY_ADDRESS: u8 = 0x23;
    /// SecurityAccess.
    pub const SECURITY_ACCESS: u8 = 0x27;
    /// CommunicationControl.
    pub const COMMUNICATION_CONTROL: u8 = 0x28;
    /// WriteDataByIdentifier.
    pub const WRITE_DATA_BY_IDENTIFIER: u8 = 0x2E;
    /// InputOutputControlByIdentifier.
    pub const INPUT_OUTPUT_CONTROL_BY_IDENTIFIER: u8 = 0x2F;
    /// RoutineControl.
    pub const ROUTINE_CONTROL: u8 = 0x31;
    /// RequestDownload.
    pub const REQUEST_DOWNLOAD: u8 = 0x34;
    /// TransferData.
    pub const TRANSFER_DATA: u8 = 0x36;
    /// RequestTransferExit.
    pub const REQUEST_TRANSFER_EXIT: u8 = 0x37;
    /// TesterPresent.
    pub const TESTER_PRESENT: u8 = 0x3E;
    /// ControlDTCSetting.
    pub const CONTROL_DTC_SETTING: u8 = 0x85;
}

/// The response codes a job returns: ISO 14229's negative response codes plus the
/// library's own `NOT_RESPONSIBLE` and `OK` (`DiagReturnCode::Type`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DiagReturnCode {
    /// generalReject.
    IsoGeneralReject = 0x10,
    /// serviceNotSupported.
    IsoServiceNotSupported = 0x11,
    /// subFunctionNotSupported.
    IsoSubfunctionNotSupported = 0x12,
    /// incorrectMessageLengthOrInvalidFormat.
    IsoInvalidFormat = 0x13,
    /// responseTooLong.
    IsoResponseTooLong = 0x14,
    /// busyRepeatRequest.
    IsoBusyRepeatRequest = 0x21,
    /// conditionsNotCorrect.
    IsoConditionsNotCorrect = 0x22,
    /// requestSequenceError.
    IsoRequestSequenceError = 0x24,
    /// noResponseFromSubnetComponent.
    IsoControlUnitOnSubbusNotResponding = 0x25,
    /// requestOutOfRange.
    IsoRequestOutOfRange = 0x31,
    /// securityAccessDenied.
    IsoSecurityAccessDenied = 0x33,
    /// authenticationRequired.
    IsoAuthenticationRequired = 0x34,
    /// invalidKey.
    IsoInvalidKey = 0x35,
    /// exceedNumberOfAttempts.
    IsoExceededNumsOfAttempts = 0x36,
    /// requiredTimeDelayNotExpired.
    IsoRequiredTimeDelayNotExpired = 0x37,
    /// uploadDownloadNotAccepted.
    IsoUploadDownloadNotAccepted = 0x70,
    /// transferDataSuspended.
    IsoTransferDataSuspended = 0x71,
    /// generalProgrammingFailure.
    IsoGeneralProgrammingFailure = 0x72,
    /// wrongBlockSequenceCounter.
    IsoWrongBlockSequenceCounter = 0x73,
    /// requestCorrectlyReceivedResponsePending.
    IsoResponsePending = 0x78,
    /// subFunctionNotSupportedInActiveSession.
    IsoSubfunctionNotSupportedInActiveSession = 0x7E,
    /// serviceNotSupportedInActiveSession.
    IsoServiceNotSupportedInActiveSession = 0x7F,
    /// The job does not handle this request; the next one is asked.
    NotResponsible = 0xFE,
    /// The job handled the request.
    Ok = 0xFF,
}

impl DiagReturnCode {
    /// The first byte of a negative response.
    pub const NEGATIVE_RESPONSE_IDENTIFIER: u8 = 0x7F;
    /// What a positive response adds to the service id.
    pub const POSITIVE_RESPONSE_OFFSET: u8 = 0x40;

    /// The code as the byte a negative response carries.
    pub const fn as_byte(self) -> u8 {
        self as u8
    }

    /// The code for a negative response byte; unknown bytes read as `IsoGeneralReject`.
    pub const fn from_byte(byte: u8) -> Self {
        match byte {
            0x10 => Self::IsoGeneralReject,
            0x11 => Self::IsoServiceNotSupported,
            0x12 => Self::IsoSubfunctionNotSupported,
            0x13 => Self::IsoInvalidFormat,
            0x14 => Self::IsoResponseTooLong,
            0x21 => Self::IsoBusyRepeatRequest,
            0x22 => Self::IsoConditionsNotCorrect,
            0x24 => Self::IsoRequestSequenceError,
            0x25 => Self::IsoControlUnitOnSubbusNotResponding,
            0x31 => Self::IsoRequestOutOfRange,
            0x33 => Self::IsoSecurityAccessDenied,
            0x34 => Self::IsoAuthenticationRequired,
            0x35 => Self::IsoInvalidKey,
            0x36 => Self::IsoExceededNumsOfAttempts,
            0x37 => Self::IsoRequiredTimeDelayNotExpired,
            0x70 => Self::IsoUploadDownloadNotAccepted,
            0x71 => Self::IsoTransferDataSuspended,
            0x72 => Self::IsoGeneralProgrammingFailure,
            0x73 => Self::IsoWrongBlockSequenceCounter,
            0x78 => Self::IsoResponsePending,
            0x7E => Self::IsoSubfunctionNotSupportedInActiveSession,
            0x7F => Self::IsoServiceNotSupportedInActiveSession,
            0xFE => Self::NotResponsible,
            0xFF => Self::Ok,
            _ => Self::IsoGeneralReject,
        }
    }
}

impl From<DiagReturnCode> for openbsw_util::format::Arg<'_> {
    fn from(code: DiagReturnCode) -> Self {
        u32::from(code.as_byte()).into()
    }
}

/// The message constants of `DiagCodes.h`.
pub struct DiagCodes;

impl DiagCodes {
    /// A positive response's code.
    pub const POSITIVE_RESPONSE: u8 = 0x00;
    /// The shortest diagnostic message.
    pub const MIN_DIAG_MESSAGE_LENGTH: u8 = 3;
    /// A negative response: `7F <service> <code>`.
    pub const NEGATIVE_RESPONSE_MESSAGE_LENGTH: u8 = 3;
    /// Where the `7F` sits.
    pub const NEGATIVE_RESPONSE_IDENTIFIER_OFFSET: u8 = 0;
    /// Where the service id sits.
    pub const NEGATIVE_RESPONSE_SERVICE_OFFSET: u8 = 1;
    /// Where the code sits.
    pub const NEGATIVE_RESPONSE_ERRORCODE_OFFSET: u8 = 2;
    /// RoutineControl startRoutine.
    pub const ID_ROUTINE_CONTROL_START_ROUTINE: u8 = 0x01;
    /// RoutineControl stopRoutine.
    pub const ID_ROUTINE_CONTROL_STOP_ROUTINE: u8 = 0x02;
    /// RoutineControl requestRoutineResults.
    pub const ID_ROUTINE_CONTROL_GET_ROUTINE_RESULT: u8 = 0x03;
    /// The functional address of every KWP2000 ECU.
    pub const FUNCTIONAL_ID_ALL_KWP2000_ECUS: u8 = 0xEF;
    /// The functional address of every ISO 14229 ECU.
    pub const FUNCTIONAL_ID_ALL_ISO14229_ECUS: u8 = 0xDF;
    /// A service id is one byte.
    pub const SERVICE_ID_LENGTH: u8 = 1;
    /// A subfunction id is one byte.
    pub const SUBFUNCTION_ID_LENGTH: u8 = 1;
    /// A routine id is two bytes.
    pub const ROUTINE_ID_LENGTH: u8 = 2;
    /// A data identifier is two bytes.
    pub const DATA_ID_LENGTH: u8 = 2;
}

/// What sending a response can fail with (`uds::ErrorCode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionErrorCode {
    /// Sent, or queued to be sent.
    Ok,
    /// The connection has no transport message to answer with.
    NoTpMessage,
    /// The transport layer refused the message.
    SendFailed,
    /// The connection was closed.
    ConnectionNotOpen,
    /// A different request is in progress.
    ConflictingRequest,
    /// A response is already being sent.
    ConnectionBusy,
}
