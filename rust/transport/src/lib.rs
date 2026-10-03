// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Port of `libs/bsw/transport` and `libs/bsw/transportRouterSimple`: the messages that
//! travel between a bus (DoCAN) and a diagnostic application (UDS), and the router that
//! hands them across.
//!
//! - [`TransportMessage`]: a message over a borrowed byte buffer (`TransportMessage.h`).
//! - [`TransportMessageListener`], [`TransportMessageProcessedListener`],
//!   [`TransportMessageProvider`]: who receives, who is told when a message is done, who
//!   hands out buffers (`ITransportMessage*.h`).
//! - [`TransportLayer`] and [`ProvidingListenerHelper`]: a bus's layer and its hooks into
//!   the router (`AbstractTransportLayer.h`).
//! - [`TransportRouterSimple`]: three large and eight small buffers, and the CAN-to-UDS
//!   round trip (`TransportRouterSimple.h`).
//! - [`logical_address`]: the 1-byte/2-byte tester address lists (`LogicalAddress.h`).
//! - [`TRANSPORT`] and [`TPROUTER`]: the logger components.
//!
//! Messages are `'static` objects (the router's) whose bytes live in cells, so the layer
//! that fills one and the application that reads it share it as the C++ does through a
//! pointer. The router is the version the Zephyr demo is built with: no 1-byte to 2-byte
//! address conversion, which the demo's `TransportConfiguration` does not define.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

mod layer;
pub mod logical_address;
mod message;
mod router;

use openbsw_util::cell::RacyCell;
use openbsw_util::logger::LoggerComponent;

pub use layer::{
    LayerErrorCode, LayerNode, ProvidingListenerHelper, SYNC_SHUTDOWN_COMPLETE, ShutdownDelegate,
    ShutdownListener, TransportLayer,
};
pub use message::{
    DefaultTransportMessageProcessedListener, MessageErrorCode, ProcessingResult,
    ProviderErrorCode, ReceiveResult, TransportMessage, TransportMessageListener,
    TransportMessageProcessedListener, TransportMessageProvider, TransportMessageProvidingListener,
};
pub use router::{RouterConfig, TransportRouterSimple};

/// The `TRANSPORT` logger component (`TransportLogger.h`).
pub static TRANSPORT: LoggerComponent = LoggerComponent::new();
/// The `TPROUTER` logger component (`TpRouterLogger.h`).
pub static TPROUTER: LoggerComponent = LoggerComponent::new();

/// Names a bus for the log: the port of `BusIdTraits::getName`.
pub type BusNameResolver = dyn Fn(u8) -> &'static [u8] + Sync;

fn default_bus_name(_bus_id: u8) -> &'static [u8] {
    b"INVALID"
}

static BUS_NAME: RacyCell<&'static BusNameResolver> = RacyCell::new(&default_bus_name);

/// Install the application's bus names (`BusIdTraits::getName`). Call during startup.
pub fn set_bus_name_resolver(resolver: &'static BusNameResolver) {
    // SAFETY: called during single-threaded startup, before any layer logs.
    unsafe { BUS_NAME.set(resolver) };
}

/// The name of `bus_id`.
pub fn bus_name(bus_id: u8) -> &'static [u8] {
    // SAFETY: set during startup, read afterwards.
    (unsafe { BUS_NAME.get() })(bus_id)
}
