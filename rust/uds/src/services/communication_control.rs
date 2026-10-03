// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Service 0x28: the port of `services/communicationcontrol/CommunicationControl.h` with
//! `ICommunicationStateManager.h`, `ICommunicationStateListener.h` and
//! `ICommunicationSubStateListener.h`.

use core::cell::Cell;

use crate::codes::{DiagReturnCode, ServiceId};
use crate::connection::IncomingDiagConnection;
use crate::job::{DiagJob, JobBase, Request, Service, VARIABLE_REQUEST_LENGTH, request_byte};
use crate::session::DiagSessionMask;

/// The communication states a listener hears about (`ICommunicationStateListener`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommunicationState {
    /// Normal messages flow both ways.
    EnableNormalMessageTransmission,
    /// Normal messages stop both ways.
    DisableNormalMessageTransmission,
    /// Normal messages are received, not sent.
    EnableRecDisableNormalMessageSendTransmission,
    /// Network management messages flow both ways.
    EnableNnMessageTransmission,
    /// Network management messages stop both ways.
    DisableNmMessageTransmission,
    /// Network management messages are received, not sent.
    EnableRecDisableNmSendTransmission,
    /// Every message flows both ways.
    EnableAllMessageTransmission,
    /// Every message stops both ways.
    DisableAllMessageTransmission,
    /// Every message is received, none sent.
    EnableRecDisableAllSendTransmission,
    /// Normal messages are sent, not received.
    DisableRecEnableNormalMessageSendTransmission,
    /// Network management messages are sent, not received.
    DisableRecEnableNmSendTransmission,
    /// Every message is sent, none received.
    DisableRecEnableAllSendTransmission,
}

/// The states of one node's enhanced-address communication
/// (`ICommunicationSubStateListener`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommunicationEnhancedState {
    /// The node's messages are received, not sent.
    EnableRecDisableEnhancedSendTransmission,
    /// The node's messages flow both ways.
    EnableEnhancedTransmission,
}

/// Hears the communication state (`ICommunicationStateListener`).
pub trait CommunicationStateListener: Sync {
    /// The state changed.
    fn communication_state_changed(&self, new_state: CommunicationState);
    /// The node that links this listener into the service's list.
    fn node(&self) -> &StateListenerNode;
}

/// Hears one node's enhanced-address state (`ICommunicationSubStateListener`).
pub trait CommunicationSubStateListener: Sync {
    /// The state of `node_id` changed; returns whether this listener handles that node.
    fn communication_state_changed(
        &self,
        new_state: CommunicationEnhancedState,
        node_id: u16,
    ) -> bool;
    /// The node that links this listener into the service's list.
    fn node(&self) -> &SubStateListenerNode;
}

/// The link of a [`CommunicationStateListener`].
pub struct StateListenerNode {
    next: Cell<Option<&'static dyn CommunicationStateListener>>,
}

// SAFETY: changed while listeners are added and removed, during startup.
unsafe impl Sync for StateListenerNode {}

impl StateListenerNode {
    /// An unlinked node.
    pub const fn new() -> Self {
        Self { next: Cell::new(None) }
    }
}

impl Default for StateListenerNode {
    fn default() -> Self {
        Self::new()
    }
}

/// The link of a [`CommunicationSubStateListener`].
pub struct SubStateListenerNode {
    next: Cell<Option<&'static dyn CommunicationSubStateListener>>,
}

// SAFETY: as `StateListenerNode`.
unsafe impl Sync for SubStateListenerNode {}

impl SubStateListenerNode {
    /// An unlinked node.
    pub const fn new() -> Self {
        Self { next: Cell::new(None) }
    }
}

impl Default for SubStateListenerNode {
    fn default() -> Self {
        Self::new()
    }
}

/// The request's control types (`CommunicationControl::ControlType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ControlType {
    /// enableRxAndTx.
    EnableRxAndTx = 0x00,
    /// enableRxAndDisableTx.
    EnableRxAndDisableTx = 0x01,
    /// disableRxAndEnableTx.
    DisableRxAndEnableTx = 0x02,
    /// disableRxAndTx.
    DisableRxAndTx = 0x03,
    /// enableRxAndDisableTxWithEnhancedAddressInformation.
    EnableRxAndDisableTxWithEnhancedAddressInformation = 0x04,
    /// enableRxAndTxWithEnhancedAddressInformation.
    EnableRxAndTxWithEnhancedAddressInformation = 0x05,
}

const NORMAL_COMMUNICATION_MESSAGES: u8 = 0x01;
const NM_COMMUNICATION_MESSAGES: u8 = 0x02;
const NO_REC_NODE_COMMUNICATION_MESSAGES: u8 = 0x00;
const REC_NODE_COMMUNICATION_MESSAGES: u8 = 0xF0;

/// CommunicationControl: keeps the communication state and tells its listeners.
pub struct CommunicationControl {
    service: Service,
    communication_state: Cell<CommunicationState>,
    sub_node_id_disabled_tx: Cell<u16>,
    never_notified: Cell<bool>,
    listeners: Cell<Option<&'static dyn CommunicationStateListener>>,
    sub_listeners: Cell<Option<&'static dyn CommunicationSubStateListener>>,
}

// SAFETY: changed on the diagnostic context, and while listeners are added at startup.
unsafe impl Sync for CommunicationControl {}

impl CommunicationControl {
    const RESPONSE_LENGTH: u8 = 1;
    const EXPECTED_REQUEST_LENGTH_EXTENDED: usize = 4;
    const EXPECTED_REQUEST_LENGTH: usize = 2;
    const VMS_CONTROL_TYPE_LO: u8 = 0x40;
    const VMS_CONTROL_TYPE_HI: u8 = 0x5F;

    /// The service, allowed in `session_mask` (the C++ default is the extended session).
    pub const fn new(session_mask: DiagSessionMask) -> Self {
        Self {
            service: Service::with_lengths(
                ServiceId::COMMUNICATION_CONTROL,
                VARIABLE_REQUEST_LENGTH,
                Self::RESPONSE_LENGTH,
                session_mask,
            )
            .with_suppress_positive_response(),
            communication_state: Cell::new(CommunicationState::DisableNormalMessageTransmission),
            sub_node_id_disabled_tx: Cell::new(0),
            never_notified: Cell::new(true),
            listeners: Cell::new(None),
            sub_listeners: Cell::new(None),
        }
    }

    /// The service with the C++ default mask, the extended session alone.
    pub const fn with_default_mask() -> Self {
        Self::new(DiagSessionMask::APPLICATION_EXTENDED_SESSION_MASK)
    }

    /// Add `listener` (`addCommunicationStateListener`).
    pub fn add_communication_state_listener(
        &self,
        listener: &'static dyn CommunicationStateListener,
    ) {
        let mut current = self.listeners.get();
        let mut last: Option<&'static dyn CommunicationStateListener> = None;
        while let Some(node) = current {
            last = Some(node);
            current = node.node().next.get();
        }
        listener.node().next.set(None);
        match last {
            None => self.listeners.set(Some(listener)),
            Some(last) => last.node().next.set(Some(listener)),
        }
    }

    /// Remove `listener` (`removeCommunicationStateListener`).
    pub fn remove_communication_state_listener(&self, listener: &dyn CommunicationStateListener) {
        let mut prev: Option<&'static dyn CommunicationStateListener> = None;
        let mut current = self.listeners.get();
        while let Some(candidate) = current {
            if core::ptr::addr_eq(candidate, listener) {
                let next = candidate.node().next.take();
                match prev {
                    None => self.listeners.set(next),
                    Some(prev) => prev.node().next.set(next),
                }
                return;
            }
            prev = Some(candidate);
            current = candidate.node().next.get();
        }
    }

    /// Add `listener` (`addCommunicationSubStateListener`).
    pub fn add_communication_sub_state_listener(
        &self,
        listener: &'static dyn CommunicationSubStateListener,
    ) {
        let mut current = self.sub_listeners.get();
        let mut last: Option<&'static dyn CommunicationSubStateListener> = None;
        while let Some(node) = current {
            last = Some(node);
            current = node.node().next.get();
        }
        listener.node().next.set(None);
        match last {
            None => self.sub_listeners.set(Some(listener)),
            Some(last) => last.node().next.set(Some(listener)),
        }
    }

    /// Remove `listener` (`removeCommunicationSubStateListener`).
    pub fn remove_communication_sub_state_listener(
        &self,
        listener: &dyn CommunicationSubStateListener,
    ) {
        let mut prev: Option<&'static dyn CommunicationSubStateListener> = None;
        let mut current = self.sub_listeners.get();
        while let Some(candidate) = current {
            if core::ptr::addr_eq(candidate, listener) {
                let next = candidate.node().next.take();
                match prev {
                    None => self.sub_listeners.set(next),
                    Some(prev) => prev.node().next.set(next),
                }
                return;
            }
            prev = Some(candidate);
            current = candidate.node().next.get();
        }
    }

    /// The state (`getCommunicationState`).
    pub fn communication_state(&self) -> CommunicationState {
        self.communication_state.get()
    }

    /// Set the state and tell the listeners when it changed, or the first time
    /// (`setCommunicationState`).
    pub fn set_communication_state(&self, state: CommunicationState) {
        if self.communication_state.get() != state || self.never_notified.get() {
            self.communication_state.set(state);
            self.never_notified.set(false);
            self.notify_listeners();
        }
    }

    /// Re-enable the node whose sending was disabled (`resetCommunicationSubState`).
    pub fn reset_communication_sub_state(&self) {
        let _ = self.notify_sub_listeners(
            self.sub_node_id_disabled_tx.get(),
            CommunicationEnhancedState::EnableEnhancedTransmission,
        );
        self.sub_node_id_disabled_tx.set(0);
    }

    /// A hook for a session check before processing; the default accepts.
    fn session_accepted(&self) -> DiagReturnCode {
        DiagReturnCode::Ok
    }

    fn notify_listeners(&self) {
        let mut current = self.listeners.get();
        while let Some(listener) = current {
            listener.communication_state_changed(self.communication_state.get());
            current = listener.node().next.get();
        }
    }

    fn notify_sub_listeners(&self, node_id: u16, new_state: CommunicationEnhancedState) -> bool {
        let mut handled = false;
        let mut current = self.sub_listeners.get();
        while let Some(listener) = current {
            if listener.communication_state_changed(new_state, node_id) {
                handled = true;
            }
            current = listener.node().next.get();
        }
        handled
    }

    fn answer(&'static self, connection: &'static IncomingDiagConnection, control_type: u8) {
        let response = connection.release_request_get_response();
        let _ = response.append_u8(control_type);
        let _ = connection.send_positive_response_internal(response.length() as u16, self);
    }
}

impl DiagJob for CommunicationControl {
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
        let mut ret = self.session_accepted();
        if ret != DiagReturnCode::Ok {
            return ret;
        }
        let control_type = request_byte(request, 0);
        let communication_type_lo = request.get(1).map_or(0, |cell| cell.get() & 0x0F);
        let communication_type_hi = request.get(1).map_or(0, |cell| cell.get() & 0xF0);
        let extended = control_type
            == ControlType::EnableRxAndDisableTxWithEnhancedAddressInformation as u8
            || control_type == ControlType::EnableRxAndTxWithEnhancedAddressInformation as u8;
        if (extended && request.len() != Self::EXPECTED_REQUEST_LENGTH_EXTENDED)
            || (control_type <= ControlType::DisableRxAndTx as u8
                && request.len() != Self::EXPECTED_REQUEST_LENGTH)
        {
            return DiagReturnCode::IsoInvalidFormat;
        }
        let pick = |normal, nm, all| match communication_type_lo {
            NORMAL_COMMUNICATION_MESSAGES => normal,
            NM_COMMUNICATION_MESSAGES => nm,
            _ => all,
        };
        match control_type {
            0x02 => {
                self.set_communication_state(pick(
                    CommunicationState::DisableRecEnableNormalMessageSendTransmission,
                    CommunicationState::DisableRecEnableNmSendTransmission,
                    CommunicationState::DisableRecEnableAllSendTransmission,
                ));
                self.answer(connection, control_type);
            }
            0x03 => {
                self.set_communication_state(pick(
                    CommunicationState::DisableNormalMessageTransmission,
                    CommunicationState::DisableNmMessageTransmission,
                    CommunicationState::DisableAllMessageTransmission,
                ));
                self.answer(connection, control_type);
            }
            0x00 => {
                self.set_communication_state(pick(
                    CommunicationState::EnableNormalMessageTransmission,
                    CommunicationState::EnableNnMessageTransmission,
                    CommunicationState::EnableAllMessageTransmission,
                ));
                if (communication_type_hi == REC_NODE_COMMUNICATION_MESSAGES
                    || communication_type_hi == NO_REC_NODE_COMMUNICATION_MESSAGES)
                    && self.sub_node_id_disabled_tx.get() != 0
                {
                    self.reset_communication_sub_state();
                }
                self.answer(connection, control_type);
            }
            0x01 => {
                self.set_communication_state(pick(
                    CommunicationState::EnableRecDisableNormalMessageSendTransmission,
                    CommunicationState::EnableRecDisableNmSendTransmission,
                    CommunicationState::EnableRecDisableAllSendTransmission,
                ));
                self.answer(connection, control_type);
            }
            0x04 => {
                if communication_type_lo == NORMAL_COMMUNICATION_MESSAGES {
                    let rcv_node =
                        u16::from_be_bytes([request_byte(request, 2), request_byte(request, 3)]);
                    if self.sub_node_id_disabled_tx.get() == 0 {
                        if self.notify_sub_listeners(
                            rcv_node,
                            CommunicationEnhancedState::EnableRecDisableEnhancedSendTransmission,
                        ) {
                            self.sub_node_id_disabled_tx.set(rcv_node);
                        } else {
                            ret = DiagReturnCode::IsoSubfunctionNotSupported;
                        }
                    } else if self.sub_node_id_disabled_tx.get() != rcv_node {
                        ret = DiagReturnCode::IsoSubfunctionNotSupported;
                    }
                }
                if ret == DiagReturnCode::Ok {
                    self.answer(connection, control_type);
                }
            }
            0x05 => {
                if communication_type_lo == NORMAL_COMMUNICATION_MESSAGES {
                    let rcv_node =
                        u16::from_be_bytes([request_byte(request, 2), request_byte(request, 3)]);
                    if rcv_node == self.sub_node_id_disabled_tx.get() {
                        if self.notify_sub_listeners(
                            rcv_node,
                            CommunicationEnhancedState::EnableEnhancedTransmission,
                        ) {
                            self.sub_node_id_disabled_tx.set(0);
                        } else {
                            ret = DiagReturnCode::IsoSubfunctionNotSupported;
                        }
                    } else {
                        ret = DiagReturnCode::IsoSubfunctionNotSupported;
                    }
                }
                if ret == DiagReturnCode::Ok {
                    self.answer(connection, control_type);
                }
            }
            _ => {
                ret = if (Self::VMS_CONTROL_TYPE_LO..=Self::VMS_CONTROL_TYPE_HI)
                    .contains(&control_type)
                {
                    DiagReturnCode::NotResponsible
                } else {
                    DiagReturnCode::IsoSubfunctionNotSupported
                };
            }
        }
        ret
    }
}
