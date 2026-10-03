// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Who may send a request: the port of `IDiagAuthenticator` and `DefaultDiagAuthenticator`.

use crate::codes::DiagReturnCode;

/// Decides whether a tester address is authenticated.
pub trait DiagAuthenticator: Sync {
    /// Whether requests from `address` are accepted.
    fn is_authenticated(&self, address: u16) -> bool;
    /// The negative response for a rejected tester.
    fn not_authenticated_return_code(&self) -> DiagReturnCode;
}

/// Accepts everyone (`DefaultDiagAuthenticator`).
pub struct DefaultDiagAuthenticator;

impl DiagAuthenticator for DefaultDiagAuthenticator {
    fn is_authenticated(&self, _address: u16) -> bool {
        true
    }

    fn not_authenticated_return_code(&self) -> DiagReturnCode {
        DiagReturnCode::IsoAuthenticationRequired
    }
}

/// The authenticator every job uses unless it has its own
/// (`AbstractDiagJob::getDefaultDiagAuthenticator`).
pub static DEFAULT_DIAG_AUTHENTICATOR: DefaultDiagAuthenticator = DefaultDiagAuthenticator;

#[cfg(test)]
mod tests {
    use super::*;

    // Ported from DefaultDiagAuthenticatorTest.cpp.
    #[test]
    fn default_authenticator_accepts_everyone() {
        assert!(DEFAULT_DIAG_AUTHENTICATOR.is_authenticated(0x10));
        assert_eq!(
            DEFAULT_DIAG_AUTHENTICATOR.not_authenticated_return_code(),
            DiagReturnCode::IsoAuthenticationRequired
        );
    }
}
