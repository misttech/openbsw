// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Port of `libs/bsw/cpp2can`: OpenBSW's CAN abstraction.
//!
//! - [`CanFrame`] and [`can_id`]: a classic CAN frame of up to 8 bytes and the id encoding
//!   (`canframes/`).
//! - [`filter`]: the id filters a listener declares and the merger that folds them into a
//!   transceiver's acceptance mask (`filter/`).
//! - [`CanFrameListener`], [`CanFrameSentListener`], [`FilteredCanFrameSentListener`]: who
//!   is told about received and sent frames (`framemgmt/`, `canframes/`).
//! - [`CanTransceiver`] and [`AbstractCanTransceiver`]: the transceiver interface and the
//!   listener bookkeeping every transceiver shares (`transceiver/`).
//! - [`CAN`]: the logger component (`CanLogger.h`).
//!
//! Not ported: `statistics.h` (`get_statistics` is defined per platform and the Zephyr
//! build has none), and the `AbstractBitFieldFilteredCANFrameListener` and
//! `AbstractIntervalFilteredCANFrameListener` bases, since a Rust listener embeds the
//! filter it returns from [`CanFrameListener::filter`].

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

pub mod can_id;
pub mod filter;
mod frame;
mod listener;
mod transceiver;

use openbsw_util::logger::LoggerComponent;

pub use frame::CanFrame;
pub use listener::{
    CanFrameListener, CanFrameSentListener, FilteredCanFrameSentListener, ListenerNode,
    SentListenerNode,
};
pub use transceiver::{
    AbstractCanTransceiver, BAUDRATE_HIGHSPEED, BAUDRATE_LOWSPEED, CanTransceiver,
    CanTransceiverStateListener, ErrorCode, INVALID_FRAME_ID, RX_QUEUE_SIZE, State,
    TransceiverState,
};

/// The `CAN` logger component (`DECLARE_LOGGER_COMPONENT(CAN)`).
pub static CAN: LoggerComponent = LoggerComponent::new();
