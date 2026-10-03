// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! A transport message and who handles it, ported from `TransportMessage.h`, `.cpp`,
//! `ITransportMessageListener.h`, `ITransportMessageProcessedListener.h`,
//! `ITransportMessageProvider.h` and `ITransportMessageProvidingListener.h`.

use core::cell::Cell;

use openbsw_util::log_critical;

use crate::TRANSPORT;

/// What appending to a message reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageErrorCode {
    /// Done.
    Ok,
    /// The buffer is full.
    LengthExceeded,
}

const EMPTY: &[Cell<u8>] = &[];

/// A message over a borrowed buffer: source and target addresses, a payload length, and
/// how many payload bytes are valid so far.
///
/// The bytes are cells: a message is a `'static` object the bus layer fills and the
/// application reads, as the C++ code shares it through a pointer.
pub struct TransportMessage {
    buffer: Cell<&'static [Cell<u8>]>,
    source_address: Cell<u16>,
    target_address: Cell<u16>,
    payload_length: Cell<u16>,
    valid_bytes: Cell<u16>,
}

// SAFETY: a message is handed from one owner to the next (provider, receiver,
// application, back); only its current owner touches it, the contract the C++ pointer
// handover relies on.
unsafe impl Sync for TransportMessage {}

impl TransportMessage {
    /// Where the service id is in the payload.
    pub const SERVICE_ID_INDEX: u16 = 0;
    /// An address that is none.
    pub const INVALID_ADDRESS: u16 = 0xFFFF;

    /// A message without a buffer.
    pub const fn new() -> Self {
        Self::with_buffer(EMPTY)
    }

    /// A message over `buffer`.
    pub const fn with_buffer(buffer: &'static [Cell<u8>]) -> Self {
        Self {
            buffer: Cell::new(buffer),
            source_address: Cell::new(Self::INVALID_ADDRESS),
            target_address: Cell::new(Self::INVALID_ADDRESS),
            payload_length: Cell::new(0),
            valid_bytes: Cell::new(0),
        }
    }

    /// Use `buffer` from now on, with nothing valid in it.
    pub fn init(&self, buffer: &'static [Cell<u8>]) {
        self.buffer.set(buffer);
        self.valid_bytes.set(0);
        if !buffer.is_empty() {
            self.set_payload_length(0);
        }
    }

    /// The source address (`getSourceId`).
    pub fn source_id(&self) -> u16 {
        self.source_address.get()
    }

    /// Set the source address.
    pub fn set_source_address(&self, source_address: u16) {
        self.source_address.set(source_address);
    }

    /// The target address (`getTargetId`).
    pub fn target_id(&self) -> u16 {
        self.target_address.get()
    }

    /// Set the target address.
    pub fn set_target_address(&self, target_address: u16) {
        self.target_address.set(target_address);
    }

    /// The first payload byte.
    ///
    /// # Panics
    ///
    /// Without a buffer (the C++ indexes an empty span).
    pub fn service_id(&self) -> u8 {
        self.byte(Self::SERVICE_ID_INDEX)
    }

    /// Set the first payload byte, making it valid if nothing was.
    ///
    /// # Panics
    ///
    /// Without a buffer (the C++ asserts).
    pub fn set_service_id(&self, service_id: u8) {
        if self.buffer.get().is_empty() {
            log_critical!(TRANSPORT, b"TransportMessage::setServiceId(): fpBuffer is NULL!");
            panic!("buffer size is zero");
        }
        self.set_byte(Self::SERVICE_ID_INDEX, service_id);
        if self.valid_bytes.get() == 0 {
            let _ = self.increase_valid_bytes(1);
        }
    }

    /// The whole buffer (`getBuffer`).
    pub fn buffer(&self) -> &'static [Cell<u8>] {
        self.buffer.get()
    }

    /// The buffer's size.
    pub fn buffer_length(&self) -> u32 {
        self.buffer.get().len() as u32
    }

    /// The payload: the buffer up to the payload length (`getPayload`).
    pub fn payload(&self) -> &'static [Cell<u8>] {
        &self.buffer.get()[..usize::from(self.payload_length.get())]
    }

    /// Byte `pos` of the buffer (`operator[]`).
    pub fn byte(&self, pos: u16) -> u8 {
        self.buffer.get()[usize::from(pos)].get()
    }

    /// Set byte `pos` of the buffer (`operator[]`).
    pub fn set_byte(&self, pos: u16, value: u8) {
        self.buffer.get()[usize::from(pos)].set(value);
    }

    /// Copy the valid bytes into `dst`; returns how many were copied.
    pub fn read(&self, offset: u16, dst: &mut [u8]) -> usize {
        let buffer = self.buffer.get();
        let start = usize::from(offset).min(buffer.len());
        let end = usize::from(self.valid_bytes.get()).max(start).min(buffer.len());
        let count = (end - start).min(dst.len());
        for (byte, cell) in dst.iter_mut().zip(&buffer[start..start + count]) {
            *byte = cell.get();
        }
        count
    }

    /// The payload length.
    pub fn payload_length(&self) -> u16 {
        self.payload_length.get()
    }

    /// Set the payload length.
    ///
    /// # Panics
    ///
    /// If `length` exceeds the buffer (the C++ asserts).
    pub fn set_payload_length(&self, length: u16) {
        if length > self.max_payload_length() {
            log_critical!(
                TRANSPORT,
                b"TransportMessage::setPayloadLength(): length is too large (%d), maxLength is:%d!",
                length,
                self.max_payload_length()
            );
            panic!("length is too large");
        }
        self.payload_length.set(length);
    }

    /// The buffer's size as a payload length.
    pub fn max_payload_length(&self) -> u16 {
        self.buffer.get().len() as u16
    }

    /// Append `data` after the valid bytes.
    pub fn append(&self, data: &[u8]) -> MessageErrorCode {
        let valid = usize::from(self.valid_bytes.get());
        if valid + data.len() > usize::from(self.max_payload_length()) {
            return MessageErrorCode::LengthExceeded;
        }
        for (cell, &byte) in self.buffer.get()[valid..].iter().zip(data) {
            cell.set(byte);
        }
        self.increase_valid_bytes(data.len() as u16)
    }

    /// Append one byte after the valid bytes.
    pub fn append_byte(&self, data: u8) -> MessageErrorCode {
        let valid = usize::from(self.valid_bytes.get());
        if valid + 1 > usize::from(self.max_payload_length()) {
            return MessageErrorCode::LengthExceeded;
        }
        self.buffer.get()[valid].set(data);
        self.increase_valid_bytes(1)
    }

    /// No byte is valid.
    pub fn reset_valid_bytes(&self) {
        self.valid_bytes.set(0);
    }

    /// `n` more bytes are valid; past the buffer, all of it is and the excess reported.
    pub fn increase_valid_bytes(&self, n: u16) -> MessageErrorCode {
        let total = u32::from(self.valid_bytes.get()) + u32::from(n);
        if total > u32::from(self.max_payload_length()) {
            self.valid_bytes.set(self.max_payload_length());
            return MessageErrorCode::LengthExceeded;
        }
        self.valid_bytes.set(total as u16);
        MessageErrorCode::Ok
    }

    /// How many bytes are valid.
    pub fn valid_bytes(&self) -> u16 {
        self.valid_bytes.get()
    }

    /// How many payload bytes are not valid yet.
    pub fn missing_bytes(&self) -> u16 {
        self.payload_length.get().wrapping_sub(self.valid_bytes.get())
    }

    /// Whether every payload byte is valid.
    pub fn is_complete(&self) -> bool {
        self.valid_bytes.get() >= self.payload_length.get()
    }
}

impl Default for TransportMessage {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for TransportMessage {
    fn eq(&self, other: &Self) -> bool {
        if self.payload_length.get() != other.payload_length.get()
            || self.valid_bytes.get() != other.valid_bytes.get()
        {
            return false;
        }
        if self.target_id() != other.target_id() || self.source_id() != other.source_id() {
            return false;
        }
        let valid = usize::from(self.valid_bytes.get());
        self.buffer.get()[..valid]
            .iter()
            .zip(&other.buffer.get()[..valid])
            .all(|(a, b)| a.get() == b.get())
    }
}

impl core::fmt::Debug for TransportMessage {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TransportMessage")
            .field("source", &self.source_id())
            .field("target", &self.target_id())
            .field("payload_length", &self.payload_length.get())
            .field("valid_bytes", &self.valid_bytes.get())
            .finish()
    }
}

/// What receiving a message reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiveResult {
    /// Taken.
    NoError,
    /// Refused.
    Error,
}

/// How a message was processed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessingResult {
    /// Done.
    NoError,
    /// A timeout.
    ErrorTimeout,
    /// An overflow.
    ErrorOverflow,
    /// Aborted.
    ErrorAbort,
    /// Some other error (`PROCESSED_ERROR`).
    ErrorGeneral,
}

/// Receives messages: the port of `ITransportMessageListener`.
pub trait TransportMessageListener: Sync {
    /// `message` arrived on `source_bus_id`; `notification` is told when it is processed.
    fn message_received(
        &self,
        source_bus_id: u8,
        message: &'static TransportMessage,
        notification: Option<&'static dyn TransportMessageProcessedListener>,
    ) -> ReceiveResult;
}

/// Told when a message it handed out is processed: the port of
/// `ITransportMessageProcessedListener`.
pub trait TransportMessageProcessedListener: Sync {
    /// `message` was processed with `result`.
    fn transport_message_processed(
        &self,
        message: &'static TransportMessage,
        result: ProcessingResult,
    );
}

/// A listener that does nothing.
pub struct DefaultTransportMessageProcessedListener;

impl TransportMessageProcessedListener for DefaultTransportMessageProcessedListener {
    fn transport_message_processed(
        &self,
        _message: &'static TransportMessage,
        _result: ProcessingResult,
    ) {
    }
}

/// Why a provider refused a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderErrorCode {
    /// The source address is not served.
    InvalidSrcAddress,
    /// The target address is not served.
    InvalidTgtAddress,
    /// Every buffer is in use.
    NoMsgAvailable,
    /// No buffer is that large.
    SizeTooLarge,
    /// Not this provider's business.
    NotResponsible,
}

/// Hands out message buffers: the port of `ITransportMessageProvider`.
pub trait TransportMessageProvider: Sync {
    /// A message for `size` bytes from `source_address` to `target_address`, received on
    /// `src_bus_id`; `peek` is the start of the data.
    fn get_transport_message(
        &self,
        src_bus_id: u8,
        source_address: u16,
        target_address: u16,
        size: u16,
        peek: &[u8],
    ) -> Result<&'static TransportMessage, ProviderErrorCode>;
    /// `message` is no longer used.
    fn release_transport_message(&self, message: &'static TransportMessage);
    /// Print the provider's state.
    fn dump(&self) {}
}

/// Both at once: the port of `ITransportMessageProvidingListener`.
pub trait TransportMessageProvidingListener:
    TransportMessageListener + TransportMessageProvider
{
}

impl<T: TransportMessageListener + TransportMessageProvider> TransportMessageProvidingListener
    for T
{
}

// Ported from transport/test/src/TransportMessageTest.cpp.
#[cfg(test)]
mod tests {
    extern crate std;

    use std::boxed::Box;

    use super::*;

    const BUFFER_LENGTH: usize = 16;

    fn buffer() -> &'static [Cell<u8>] {
        Box::leak(Box::new([const { Cell::new(0) }; BUFFER_LENGTH]))
    }

    fn filled(value: u8) -> &'static [Cell<u8>] {
        let buffer = buffer();
        for cell in buffer {
            cell.set(value);
        }
        buffer
    }

    fn message() -> TransportMessage {
        TransportMessage::with_buffer(buffer())
    }

    #[test]
    #[should_panic(expected = "buffer size is zero")]
    fn set_service_id_without_buffer_asserts() {
        TransportMessage::new().set_service_id(1);
    }

    #[test]
    #[should_panic(expected = "length is too large")]
    fn set_payload_length_beyond_the_buffer_asserts() {
        TransportMessage::new().set_payload_length(100);
    }

    #[test]
    fn default_constructor() {
        let m = TransportMessage::new();
        assert!(m.buffer().is_empty());
        assert_eq!(m.valid_bytes(), 0);
        assert_eq!(m.source_id(), TransportMessage::INVALID_ADDRESS);
    }

    #[test]
    fn init() {
        let m = message();
        let buffer = buffer();
        m.init(buffer);
        assert!(core::ptr::eq(m.buffer(), buffer));
        assert_eq!(m.buffer_length(), BUFFER_LENGTH as u32);
        assert_eq!(m.valid_bytes(), 0);
        assert_eq!(m.payload_length(), 0);
        m.init(&buffer[..0]);
        assert_eq!(m.buffer_length(), 0);
        m.init(EMPTY);
        assert!(m.buffer().is_empty());
        assert_eq!(m.missing_bytes(), 0);
    }

    #[test]
    fn get_set_source_id() {
        let m = message();
        m.set_source_address(2);
        assert_eq!(m.source_id(), 2);
        m.set_source_address(0xFFFF);
        assert_eq!(m.source_id(), 0xFFFF);
    }

    #[test]
    fn get_set_target_id() {
        let m = message();
        m.set_target_address(42);
        assert_eq!(m.target_id(), 42);
        m.set_target_address(0xFFFF);
        assert_eq!(m.target_id(), 0xFFFF);
    }

    #[test]
    fn get_set_service_id() {
        let m = message();
        m.set_service_id(0);
        assert_eq!(m.service_id(), 0);
        assert_eq!(m.valid_bytes(), 1);
        m.set_service_id(0xFF);
        assert_eq!(m.service_id(), 0xFF);
        assert_eq!(m.valid_bytes(), 1);
    }

    #[test]
    fn get_reset_valid_bytes() {
        let m = message();
        let data: [u8; BUFFER_LENGTH] = core::array::from_fn(|i| i as u8);
        assert_eq!(m.append(&data), MessageErrorCode::Ok);
        assert_eq!(m.valid_bytes(), BUFFER_LENGTH as u16);
        let mut copy = [0; BUFFER_LENGTH];
        assert_eq!(m.read(0, &mut copy), BUFFER_LENGTH);
        assert_eq!(copy, data);
        m.reset_valid_bytes();
        assert_eq!(m.valid_bytes(), 0);
        assert_eq!(m.read(0, &mut copy), 0);
    }

    #[test]
    fn operator_index() {
        let m = message();
        m.set_byte(0, 10);
        assert_eq!(m.byte(0), 10);
    }

    #[test]
    fn is_complete() {
        let m = message();
        let size = BUFFER_LENGTH as u16;
        m.set_payload_length(size);
        assert_eq!(m.payload_length(), size);
        for i in 0..size {
            assert_eq!(m.increase_valid_bytes(i), MessageErrorCode::Ok);
            assert!(!m.is_complete());
            m.reset_valid_bytes();
        }
        assert_eq!(m.increase_valid_bytes(size), MessageErrorCode::Ok);
        assert!(m.is_complete());
        assert_eq!(m.payload().len(), BUFFER_LENGTH);
    }

    #[test]
    fn get_max_payload_length() {
        assert_eq!(message().max_payload_length(), BUFFER_LENGTH as u16);
    }

    #[test]
    fn increase_valid_bytes() {
        let m = message();
        assert_eq!(m.increase_valid_bytes(1), MessageErrorCode::Ok);
        assert_eq!(m.valid_bytes(), 1);
        m.reset_valid_bytes();
        assert_eq!(m.increase_valid_bytes(m.max_payload_length()), MessageErrorCode::Ok);
        assert_eq!(m.valid_bytes(), m.max_payload_length());
        assert_eq!(m.increase_valid_bytes(1), MessageErrorCode::LengthExceeded);
    }

    #[test]
    fn set_get_payload_length() {
        let m = message();
        m.set_payload_length(m.max_payload_length());
        assert_eq!(m.payload_length(), m.max_payload_length());
    }

    #[test]
    fn append() {
        let m = message();
        m.set_target_address(0x1);
        m.set_source_address(0x2);
        let data = [0, 1, 2, 3, 4, 5, 6, 7];
        assert_eq!(m.append(&[0; 17]), MessageErrorCode::LengthExceeded);
        assert_eq!(m.append(&data), MessageErrorCode::Ok);
        assert_eq!(m.valid_bytes(), 8);
        assert_eq!(m.append(&data), MessageErrorCode::Ok);
        assert_eq!(m.valid_bytes(), 16);
        let mut copy = [0; 16];
        m.read(0, &mut copy);
        assert_eq!(&copy[..8], &data);
        assert_eq!(&copy[8..], &data);
        assert_eq!(m.target_id(), 0x1);
        assert_eq!(m.source_id(), 0x2);
        assert_eq!(m.append_byte(0x00), MessageErrorCode::LengthExceeded);
    }

    #[test]
    fn compare_operator() {
        let size = BUFFER_LENGTH as u16;
        let local = TransportMessage::with_buffer(filled(0xAB));
        let m = TransportMessage::with_buffer(filled(0xAB));
        for message in [&local, &m] {
            message.set_source_address(0xC0);
            message.set_target_address(0xC1);
            message.set_payload_length(size);
        }
        assert!(m == local);
        local.set_payload_length(size - 1);
        assert!(m != local);
        local.set_payload_length(size);
        assert_eq!(m, local);
        m.reset_valid_bytes();
        local.reset_valid_bytes();
        assert_eq!(m, local);
        m.increase_valid_bytes(size - 1);
        local.increase_valid_bytes(size);
        assert!(m != local);
        m.reset_valid_bytes();
        local.reset_valid_bytes();
        m.set_target_address(0xF1);
        local.set_target_address(0xF2);
        assert!(m != local);
        local.set_target_address(0xF1);
        assert_eq!(m, local);
        m.set_source_address(0xAB);
        local.set_source_address(0xCD);
        assert!(m != local);
        local.set_source_address(0xAB);
        assert_eq!(m, local);
        for i in 0..size as u8 {
            m.append_byte(i);
            local.append_byte(i + 1);
        }
        assert!(m != local);
    }

    #[test]
    fn compare_same_everything() {
        let local = TransportMessage::with_buffer(filled(0xAB));
        let m = TransportMessage::with_buffer(filled(0xAB));
        for message in [&local, &m] {
            message.set_source_address(0xC0);
            message.set_target_address(0xC1);
        }
        assert_eq!(m, local);
        for i in 0..BUFFER_LENGTH as u8 {
            m.append_byte(i);
            local.append_byte(i);
        }
        assert_eq!(m, local);
    }
}
