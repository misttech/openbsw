// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Port of `libs/bsw/docan`: ISO 15765-2 transport over CAN, the way the demo uses it,
//! with normal addressing on classic CAN.
//!
//! - [`common`]: frame and flow status kinds, the parameters, the address pairs, the
//!   connection and the job handle.
//! - [`codec`]: how a message is cut into single, first, consecutive and flow control
//!   frames and back ([`FrameCodec`] over a [`FrameCodecConfig`]), and
//!   [`decoder::decode_frame`], which hands a decoded frame to a [`FrameReceiver`].
//! - [`addressing`]: normal addressing (the CAN id is the address) and the
//!   [`NormalAddressingFilter`] that maps CAN ids to transport addresses and codecs.
//! - [`transceiver`]: the interfaces to the CAN side and [`PhysicalCanTransceiver`], which
//!   puts them over a `cpp2can` transceiver.
//! - [`receiver`] and [`transmitter`]: one state machine per message in flight, the
//!   receivers and transmitters that hold them, with their timeouts and flow control.
//! - [`layer`]: [`DoCanTransportLayer`], the transport layer the router sees, its
//!   [`DoCanTransportLayerConfig`] and the container of several layers.
//! - [`DOCAN`]: the logger component.
//!
//! The C++ library is generic over the data link layer's address, message size, frame size
//! and frame index types; this port fixes them to the demo's: a 32-bit CAN id, 16-bit
//! message sizes, 8-bit frame sizes and 16-bit frame indices. Pools of receivers and
//! transmitters are fixed arrays of slots, as full as the C++ pools were. Not ported:
//! extended, normal fixed and range addressing.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

pub mod addressing;
pub mod codec;
pub mod common;
pub mod decoder;
pub mod layer;
pub mod receiver;
pub mod transceiver;
pub mod transmitter;

use openbsw_util::logger::LoggerComponent;

pub use addressing::{AddressConverter, AddressEntry, NormalAddressingFilter};
pub use codec::{FrameCodec, FrameCodecConfig, FrameSizeMapper};
pub use common::{
    Address, Connection, DataLinkAddressPair, DoCanParameters, FlowStatus, FrameIndex, FrameSize,
    FrameType, INVALID_ADDRESS, JobHandle, MessageSize, TransportAddressPair,
};
pub use layer::{DoCanTransportLayer, DoCanTransportLayerConfig, DoCanTransportLayerContainer};
pub use transceiver::{FrameReceiver, PhysicalCanTransceiver, PhysicalTransceiver, TickGenerator};

/// The `DOCAN` logger component (`DoCanLogger.h`).
pub static DOCAN: LoggerComponent = LoggerComponent::new();
