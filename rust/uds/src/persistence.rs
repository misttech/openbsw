// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Where the session survives a reset: the port of `ISessionPersistence.h`.

use crate::services::session_control::DiagnosticSessionControl;

/// Reads and writes the persisted session; the demo's implementation does nothing.
pub trait SessionPersistence: Sync {
    /// Read the session and call `session_control.session_read` with it.
    fn read_session(&self, session_control: &'static DiagnosticSessionControl);
    /// Write `session` and call `session_control.session_written`.
    fn write_session(&self, session_control: &'static DiagnosticSessionControl, session: u8);
}
