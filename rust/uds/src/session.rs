// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The diagnostic sessions, ported from the demo's `uds/session/DiagSession.h`, the
//! library's `ApplicationDefaultSession`, `ApplicationExtendedSession` and
//! `ProgrammingSession`, `util/Mask.h`, and `IDiagSessionManager.h`.

use core::cell::Cell;

use crate::codes::DiagReturnCode;
use crate::connection::IncomingDiagConnection;
use crate::job::JobBase;

/// The session kinds, as the DiagnosticSessionControl request names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum SessionType {
    /// defaultSession.
    Default = 0x01,
    /// programmingSession.
    Programming = 0x02,
    /// extendedDiagnosticSession.
    Extended = 0x03,
}

impl SessionType {
    /// The kind a request byte names, if any.
    pub const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0x01 => Some(Self::Default),
            0x02 => Some(Self::Programming),
            0x03 => Some(Self::Extended),
            _ => None,
        }
    }
}

/// A diagnostic session: the port of `DiagSession` and of the three concrete sessions,
/// whose transition rules are in one place here.
#[derive(Debug)]
pub struct DiagSession {
    session_type: SessionType,
    index: u8,
}

impl PartialEq for DiagSession {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index
    }
}

/// The highest session index plus one (`DiagSession::MAX_INDEX`).
pub const MAX_SESSION_INDEX: u8 = 8;

/// `DiagSession::APPLICATION_DEFAULT_SESSION()`.
pub static APPLICATION_DEFAULT_SESSION: DiagSession = DiagSession::new(SessionType::Default, 0x01);
/// `DiagSession::APPLICATION_EXTENDED_SESSION()`.
pub static APPLICATION_EXTENDED_SESSION: DiagSession =
    DiagSession::new(SessionType::Extended, 0x03);
/// `DiagSession::PROGRAMMING_SESSION()`.
pub static PROGRAMMING_SESSION: DiagSession = DiagSession::new(SessionType::Programming, 0x04);

impl DiagSession {
    const fn new(session_type: SessionType, index: u8) -> Self {
        Self { session_type, index }
    }

    /// The bit of this session in a mask (`toIndex`).
    pub const fn to_index(&self) -> u8 {
        self.index
    }

    /// The session byte of DiagnosticSessionControl (`getSessionByte`).
    pub const fn session_byte(&self) -> u8 {
        self.session_type as u8
    }

    /// The kind (`getType`).
    pub const fn session_type(&self) -> SessionType {
        self.session_type
    }

    /// Whether this session may switch to `target` (`isTransitionPossible`).
    pub fn is_transition_possible(&self, target: SessionType) -> DiagReturnCode {
        match (self.session_type, target) {
            (SessionType::Default, SessionType::Default | SessionType::Extended) => {
                DiagReturnCode::Ok
            }
            (SessionType::Default, SessionType::Programming) => {
                DiagReturnCode::IsoSubfunctionNotSupportedInActiveSession
            }
            (SessionType::Extended, _) => DiagReturnCode::Ok,
            (SessionType::Programming, SessionType::Default | SessionType::Programming) => {
                DiagReturnCode::Ok
            }
            (SessionType::Programming, SessionType::Extended) => {
                DiagReturnCode::IsoSubfunctionNotSupported
            }
        }
    }

    /// The session a switch to `target` ends in (`getTransitionResult`): the target where
    /// the C++ session knows it, else this session.
    pub fn transition_result(&'static self, target: SessionType) -> &'static DiagSession {
        match (self.session_type, target) {
            (SessionType::Default, SessionType::Extended) => &APPLICATION_EXTENDED_SESSION,
            (SessionType::Extended, SessionType::Default) => &APPLICATION_DEFAULT_SESSION,
            (SessionType::Extended, SessionType::Programming) => &PROGRAMMING_SESSION,
            (SessionType::Programming, SessionType::Default) => &APPLICATION_DEFAULT_SESSION,
            _ => self,
        }
    }

    /// Called when the session becomes active (`enter`); nothing happens.
    pub fn enter(&self) {}
}

/// A set of sessions: the port of `Mask<DiagSession>`, one bit per session index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiagSessionMask(u8);

impl DiagSessionMask {
    /// No session (`Mask::getInstance()`, which is cleared).
    pub const EMPTY: Self = Self(0);
    /// Every session (`DiagSession::ALL_SESSIONS()`).
    pub const ALL_SESSIONS: Self = Self(0xFF);
    /// The extended session alone (`DiagSession::APPLICATION_EXTENDED_SESSION_MASK()`).
    pub const APPLICATION_EXTENDED_SESSION_MASK: Self = Self(1 << 0x03);
    /// The default session alone.
    pub const APPLICATION_DEFAULT_SESSION_MASK: Self = Self(1 << 0x01);

    /// A mask of `bits`.
    pub const fn new(bits: u8) -> Self {
        Self(bits)
    }

    /// The bits.
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// This mask with `session` added (`operator<<`).
    pub const fn with(self, session: &DiagSession) -> Self {
        Self(self.0 | (1 << session.to_index()))
    }

    /// This mask with `session` removed (`operator>>`).
    pub const fn without(self, session: &DiagSession) -> Self {
        Self(self.0 & !(1 << session.to_index()))
    }

    /// Whether `session` is in the mask (`match`).
    pub const fn matches(self, session: &DiagSession) -> bool {
        self.0 & (1 << session.to_index()) != 0
    }
}

/// Told when the active session changes or a response goes out
/// (`IDiagSessionChangedListener`), kept in the session manager's intrusive list.
pub trait DiagSessionChangedListener: Sync {
    /// `session` is active now.
    fn diag_session_changed(&self, session: &'static DiagSession);
    /// A response with `response_code` was sent.
    fn diag_session_response_sent(&self, response_code: u8);
    /// The node that links this listener into the manager's list.
    fn node(&self) -> &SessionListenerNode;
}

/// The link of a [`DiagSessionChangedListener`] in the manager's list.
pub struct SessionListenerNode {
    pub(crate) next: Cell<Option<&'static dyn DiagSessionChangedListener>>,
}

// SAFETY: changed under the platform lock while listeners are added and removed.
unsafe impl Sync for SessionListenerNode {}

impl SessionListenerNode {
    /// An unlinked node.
    pub const fn new() -> Self {
        Self { next: Cell::new(None) }
    }
}

impl Default for SessionListenerNode {
    fn default() -> Self {
        Self::new()
    }
}

/// Owns the active session and its timeout: the port of `IDiagSessionManager`. Jobs reach
/// it through [`crate::job::default_diag_session_manager`] or their own reference, and
/// `'static` because it schedules timeouts.
pub trait DiagSessionManager: Sync {
    /// The session in force.
    fn active_session(&self) -> &'static DiagSession;
    /// Arm the session timeout.
    fn start_session_timeout(&'static self);
    /// Disarm the session timeout.
    fn stop_session_timeout(&'static self);
    /// Whether a session timeout is armed.
    fn is_session_timeout_active(&self) -> bool;
    /// Fall back to the default session.
    fn reset_to_default_session(&'static self);
    /// Keep the session across a reset; the default does not.
    fn persist_and_restore_session(&self) -> bool {
        false
    }
    /// `job` accepted `request` on `connection`.
    fn accepted_job(
        &'static self,
        connection: &IncomingDiagConnection,
        job: &JobBase,
        request: &[core::cell::Cell<u8>],
    ) -> DiagReturnCode;
    /// A response with `result` and `response` bytes went out on `connection`.
    fn response_sent(
        &'static self,
        connection: &IncomingDiagConnection,
        result: DiagReturnCode,
        response: &[core::cell::Cell<u8>],
    );
    /// Add `listener`.
    fn add_diag_session_listener(&self, listener: &'static dyn DiagSessionChangedListener);
    /// Remove `listener`.
    fn remove_diag_session_listener(&self, listener: &'static dyn DiagSessionChangedListener);
}

// Ported from the session rules of ApplicationDefaultSession.cpp, ApplicationExtendedSession.cpp
// and ProgrammingSession.cpp, and from the demo's DiagSession.cpp.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_have_their_bytes_and_indices() {
        assert_eq!(APPLICATION_DEFAULT_SESSION.session_byte(), 0x01);
        assert_eq!(APPLICATION_DEFAULT_SESSION.to_index(), 0x01);
        assert_eq!(APPLICATION_EXTENDED_SESSION.session_byte(), 0x03);
        assert_eq!(APPLICATION_EXTENDED_SESSION.to_index(), 0x03);
        assert_eq!(PROGRAMMING_SESSION.session_byte(), 0x02);
        assert_eq!(PROGRAMMING_SESSION.to_index(), 0x04);
        assert!(APPLICATION_DEFAULT_SESSION != APPLICATION_EXTENDED_SESSION);
        assert_eq!(SessionType::from_byte(0x03), Some(SessionType::Extended));
        assert_eq!(SessionType::from_byte(0x04), None);
    }

    #[test]
    fn masks_match_sessions() {
        assert!(DiagSessionMask::ALL_SESSIONS.matches(&PROGRAMMING_SESSION));
        assert!(
            DiagSessionMask::APPLICATION_EXTENDED_SESSION_MASK
                .matches(&APPLICATION_EXTENDED_SESSION)
        );
        assert!(
            !DiagSessionMask::APPLICATION_EXTENDED_SESSION_MASK
                .matches(&APPLICATION_DEFAULT_SESSION)
        );
        assert!(!DiagSessionMask::EMPTY.matches(&APPLICATION_DEFAULT_SESSION));
        let mask = DiagSessionMask::EMPTY.with(&APPLICATION_DEFAULT_SESSION);
        assert_eq!(mask, DiagSessionMask::APPLICATION_DEFAULT_SESSION_MASK);
        assert_eq!(mask.without(&APPLICATION_DEFAULT_SESSION), DiagSessionMask::EMPTY);
    }

    #[test]
    fn default_session_transitions() {
        let cut = &APPLICATION_DEFAULT_SESSION;
        assert_eq!(cut.is_transition_possible(SessionType::Default), DiagReturnCode::Ok);
        assert_eq!(cut.is_transition_possible(SessionType::Extended), DiagReturnCode::Ok);
        assert_eq!(
            cut.is_transition_possible(SessionType::Programming),
            DiagReturnCode::IsoSubfunctionNotSupportedInActiveSession
        );
        assert!(core::ptr::eq(
            cut.transition_result(SessionType::Extended),
            &APPLICATION_EXTENDED_SESSION
        ));
        assert!(core::ptr::eq(cut.transition_result(SessionType::Default), cut));
        assert!(core::ptr::eq(cut.transition_result(SessionType::Programming), cut));
    }

    #[test]
    fn extended_session_transitions() {
        let cut = &APPLICATION_EXTENDED_SESSION;
        for target in [SessionType::Default, SessionType::Extended, SessionType::Programming] {
            assert_eq!(cut.is_transition_possible(target), DiagReturnCode::Ok);
        }
        assert!(core::ptr::eq(
            cut.transition_result(SessionType::Default),
            &APPLICATION_DEFAULT_SESSION
        ));
        assert!(core::ptr::eq(
            cut.transition_result(SessionType::Programming),
            &PROGRAMMING_SESSION
        ));
        assert!(core::ptr::eq(cut.transition_result(SessionType::Extended), cut));
    }

    #[test]
    fn programming_session_transitions() {
        let cut = &PROGRAMMING_SESSION;
        assert_eq!(cut.is_transition_possible(SessionType::Default), DiagReturnCode::Ok);
        assert_eq!(cut.is_transition_possible(SessionType::Programming), DiagReturnCode::Ok);
        assert_eq!(
            cut.is_transition_possible(SessionType::Extended),
            DiagReturnCode::IsoSubfunctionNotSupported
        );
        assert!(core::ptr::eq(
            cut.transition_result(SessionType::Default),
            &APPLICATION_DEFAULT_SESSION
        ));
        assert!(core::ptr::eq(cut.transition_result(SessionType::Extended), cut));
    }
}
