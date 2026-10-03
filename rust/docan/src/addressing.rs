// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Normal addressing, ported from `addressing/DoCanNormalAddressing.h`,
//! `IDoCanAddressConverter.h` and `DoCanNormalAddressingFilter.h`: the CAN id is the data
//! link address, and a sorted table maps it to transport addresses and codecs.

use core::cell::Cell;

use openbsw_cpp2can::can_id;
use openbsw_cpp2can::filter::{BitFieldFilter, Filter, Merger};
use openbsw_util::format::StringWriter;
use openbsw_util::stream::StringBufferOutputStream;

use crate::codec::FrameCodec;
use crate::common::{Address, DataLinkAddressPair, TransportAddressPair};

/// The data link address a received frame has (`DoCanNormalAddressing::decodeReceptionAddress`).
pub const fn decode_reception_address(can_id: u32, _payload: &[u8]) -> Address {
    can_id
}

/// The CAN id to send to `transmission_address` with (`encodeTransmissionAddress`).
pub const fn encode_transmission_address(
    transmission_address: Address,
    _payload: &mut [u8],
) -> u32 {
    transmission_address
}

/// Maps between data link and transport addresses: the port of `IDoCanAddressConverter`.
pub trait AddressConverter: Sync {
    /// The codec and data link addresses to send with for a transport address pair.
    fn transmission_parameters(
        &self,
        transport_address_pair: &TransportAddressPair,
    ) -> Option<(&'static FrameCodec, DataLinkAddressPair)>;
    /// The codec, transport addresses and transmission address of frames received on
    /// `reception_address`.
    fn reception_parameters(
        &self,
        reception_address: Address,
    ) -> Option<(&'static FrameCodec, TransportAddressPair, Address)>;
    /// `address` written into `buffer` for the log; returns what was written.
    fn format_data_link_address<'b>(&self, address: Address, buffer: &'b mut [u8]) -> &'b [u8];
}

/// One line of the address table: the port of `DoCanNormalAddressingFilterAddressEntry`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddressEntry {
    /// The CAN id requests arrive on.
    pub can_reception_id: Address,
    /// The CAN id responses go out on.
    pub can_transmission_id: Address,
    /// The transport source of a received message (the tester).
    pub transport_source_id: u16,
    /// The transport target of a received message (this ECU).
    pub transport_target_id: u16,
    /// The codec of received frames, as an index into the codec table.
    pub reception_codec_idx: u8,
    /// The codec of sent frames, as an index into the codec table.
    pub transmission_codec_idx: u8,
}

/// The address table and the CAN filter it implies: the port of `DoCanNormalAddressingFilter`.
///
/// The entries are sorted by reception id: base ids first (they go into the bit field
/// filter), then extended ids, then entries with an invalid reception id (send-only).
pub struct NormalAddressingFilter {
    filter: BitFieldFilter,
    entries: Cell<&'static [AddressEntry]>,
    /// The range of extended entries within `entries`.
    extended: Cell<(usize, usize)>,
    codecs: Cell<&'static [Option<&'static FrameCodec>]>,
}

// SAFETY: configured by `init` during startup; read afterwards (see `cpp2can::filter`).
unsafe impl Sync for NormalAddressingFilter {}

impl NormalAddressingFilter {
    /// An empty filter; call [`init`](Self::init) before use.
    pub const fn new() -> Self {
        Self {
            filter: BitFieldFilter::new(),
            entries: Cell::new(&[]),
            extended: Cell::new((0, 0)),
            codecs: Cell::new(&[]),
        }
    }

    /// A filter over `entries` and `codecs`.
    ///
    /// # Panics
    ///
    /// As [`init`](Self::init).
    pub fn with_entries(
        entries: &'static [AddressEntry],
        codecs: &'static [Option<&'static FrameCodec>],
    ) -> Self {
        let filter = Self::new();
        filter.init(entries, codecs);
        filter
    }

    /// Take `entries` and `codecs`.
    ///
    /// # Panics
    ///
    /// If `entries` is empty, if the reception ids are not strictly ascending, or if an
    /// invalid reception id is followed by a valid one (the C++ asserts).
    pub fn init(
        &self,
        entries: &'static [AddressEntry],
        codecs: &'static [Option<&'static FrameCodec>],
    ) {
        assert!(!entries.is_empty(), "list of addresses must not be empty");
        self.entries.set(entries);
        self.codecs.set(codecs);
        let mut first_extended = entries.len();
        let mut prev_reception_id = 0;
        let mut index = 0;
        while index < entries.len() {
            let reception_id = entries[index].can_reception_id;
            if index != 0 {
                assert!(
                    prev_reception_id < reception_id,
                    "can reception id must be monotonically increasing"
                );
            }
            prev_reception_id = reception_id;
            if can_id::is_base(reception_id) {
                self.filter.add(reception_id);
            } else {
                if first_extended == entries.len() {
                    first_extended = index;
                }
                if !can_id::is_valid(reception_id) {
                    break;
                }
            }
            index += 1;
        }
        self.extended.set((first_extended, index));
        for entry in &entries[index..] {
            assert!(!can_id::is_valid(entry.can_reception_id), "can reception id must be valid");
        }
    }

    /// Codec `codec_idx` of the table, if the index is in range.
    fn frame_codec(&self, codec_idx: u8) -> Option<&'static FrameCodec> {
        self.codecs.get().get(usize::from(codec_idx)).copied().flatten()
    }

    fn find_entry_by_reception_address(
        &self,
        reception_address: Address,
        range: core::ops::Range<usize>,
    ) -> Option<&'static AddressEntry> {
        let entries = &self.entries.get()[range];
        entries
            .binary_search_by_key(&reception_address, |entry| entry.can_reception_id)
            .ok()
            .map(|index| &entries[index])
    }
}

impl Default for NormalAddressingFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl AddressConverter for NormalAddressingFilter {
    fn transmission_parameters(
        &self,
        transport_address_pair: &TransportAddressPair,
    ) -> Option<(&'static FrameCodec, DataLinkAddressPair)> {
        let source_id = transport_address_pair.target_id();
        let target_id = transport_address_pair.source_id();
        let entry = self.entries.get().iter().find(|entry| {
            entry.transport_source_id == source_id && entry.transport_target_id == target_id
        })?;
        let pair = DataLinkAddressPair::new(entry.can_reception_id, entry.can_transmission_id);
        self.frame_codec(entry.transmission_codec_idx).map(|codec| (codec, pair))
    }

    fn reception_parameters(
        &self,
        reception_address: Address,
    ) -> Option<(&'static FrameCodec, TransportAddressPair, Address)> {
        let entry =
            self.find_entry_by_reception_address(reception_address, 0..self.extended.get().1)?;
        let pair = TransportAddressPair::new(entry.transport_source_id, entry.transport_target_id);
        self.frame_codec(entry.reception_codec_idx)
            .map(|codec| (codec, pair, entry.can_transmission_id))
    }

    fn format_data_link_address<'b>(&self, address: Address, buffer: &'b mut [u8]) -> &'b [u8] {
        let length = {
            let mut stream = StringBufferOutputStream::new(buffer, b"", b"");
            StringWriter::new(&mut stream).printf(b"0x%08x", &[address.into()]);
            stream.string().len()
        };
        &buffer[..length]
    }
}

impl Filter for NormalAddressingFilter {
    fn add(&self, filter_id: u32) {
        self.filter.add(filter_id);
    }

    fn add_range(&self, from: u32, to: u32) {
        self.filter.add_range(from, to);
    }

    fn matches(&self, filter_id: u32) -> bool {
        if can_id::is_base(filter_id) {
            return self.filter.matches(filter_id);
        }
        let (first, end) = self.extended.get();
        self.find_entry_by_reception_address(filter_id, first..end).is_some()
    }

    fn clear(&self) {
        self.filter.clear();
    }

    fn open(&self) {
        self.filter.open();
    }

    fn accept_merger(&self, merger: &dyn Merger) {
        merger.merge_with_bit_field(&self.filter);
    }
}

// Ported from docan/test/src/docan/addressing/DoCanNormalAddressingTest.cpp and
// DoCanNormalAddressingFilterTest.cpp.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{FdFrameSizeMapper, presets};

    static MAPPER: FdFrameSizeMapper = FdFrameSizeMapper;
    static CODEC1: FrameCodec = FrameCodec::new(&presets::PADDED_CLASSIC, &MAPPER);
    static CODEC2: FrameCodec = FrameCodec::new(&presets::PADDED_FD, &MAPPER);
    static CODEC3: FrameCodec = FrameCodec::new(&presets::PADDED_FD, &MAPPER);
    static CODECS: [Option<&FrameCodec>; 4] = [None, Some(&CODEC1), Some(&CODEC2), Some(&CODEC3)];

    const fn entry(
        rx: Address,
        tx: Address,
        source: u16,
        target: u16,
        rx_codec: u8,
        tx_codec: u8,
    ) -> AddressEntry {
        AddressEntry {
            can_reception_id: rx,
            can_transmission_id: tx,
            transport_source_id: source,
            transport_target_id: target,
            reception_codec_idx: rx_codec,
            transmission_codec_idx: tx_codec,
        }
    }

    const fn base(id: u16) -> Address {
        can_id::base(id)
    }
    const fn extended(id: u32) -> Address {
        can_id::extended(id)
    }

    static TEST_ENTRIES: [AddressEntry; 8] = [
        entry(base(0x513), base(0x638), 0xf54, 0x83, 1, 2),
        entry(base(0x638), base(0x7ff), 0x1, 0xfe, 1, 2),
        entry(base(0x745), base(0x52), 0x55, 0x999, 1, 2),
        entry(extended(0x513), extended(0x638), 0x123, 0x987, 1, 2),
        entry(extended(0x638), extended(0x7ff), 0x456, 0x654, 2, 3),
        entry(extended(0x744), extended(0x523), 0x789, 0x321, 1, 2),
        entry(can_id::INVALID_ID, base(0x123), 0x789, 0x111, 1, 2),
        entry(can_id::INVALID_ID, base(0x124), 0x222, 0x333, 1, 2),
    ];

    #[test]
    fn normal_addressing() {
        assert_eq!(decode_reception_address(0x123, &[1, 2]), 0x123);
        let mut payload = [0u8; 8];
        assert_eq!(encode_transmission_address(0x456, &mut payload), 0x456);
    }

    #[test]
    #[should_panic(expected = "must not be empty")]
    fn init_asserts_if_entries_is_empty() {
        let _ = NormalAddressingFilter::with_entries(&[], &CODECS);
    }

    #[test]
    fn transmission_params() {
        let cut = NormalAddressingFilter::with_entries(&TEST_ENTRIES, &CODECS);
        let (codec, pair) =
            cut.transmission_parameters(&TransportAddressPair::new(0x83, 0xf54)).unwrap();
        assert!(core::ptr::eq(codec, &CODEC2));
        assert_eq!(pair, DataLinkAddressPair::new(base(0x513), base(0x638)));
        let (codec, pair) =
            cut.transmission_parameters(&TransportAddressPair::new(0xfe, 0x01)).unwrap();
        assert!(core::ptr::eq(codec, &CODEC2));
        assert_eq!(pair, DataLinkAddressPair::new(base(0x638), base(0x7ff)));
        let (codec, pair) =
            cut.transmission_parameters(&TransportAddressPair::new(0x654, 0x456)).unwrap();
        assert!(core::ptr::eq(codec, &CODEC3));
        assert_eq!(pair, DataLinkAddressPair::new(extended(0x638), extended(0x7ff)));
        let (_, pair) =
            cut.transmission_parameters(&TransportAddressPair::new(0x111, 0x789)).unwrap();
        assert_eq!(pair, DataLinkAddressPair::new(can_id::INVALID_ID, base(0x123)));
        assert!(cut.transmission_parameters(&TransportAddressPair::new(0x84, 0xf54)).is_none());
        assert!(cut.transmission_parameters(&TransportAddressPair::new(0x83, 0x5e)).is_none());
        assert!(cut.transmission_parameters(&TransportAddressPair::new(0x457, 0x123)).is_none());
    }

    #[test]
    fn reception_params() {
        let cut = NormalAddressingFilter::with_entries(&TEST_ENTRIES, &CODECS);
        let (codec, pair, tx) = cut.reception_parameters(base(0x513)).unwrap();
        assert!(core::ptr::eq(codec, &CODEC1));
        assert_eq!(tx, base(0x638));
        assert_eq!(pair, TransportAddressPair::new(0xf54, 0x83));
        let (codec, pair, tx) = cut.reception_parameters(extended(0x638)).unwrap();
        assert!(core::ptr::eq(codec, &CODEC2));
        assert_eq!(tx, extended(0x7ff));
        assert_eq!(pair, TransportAddressPair::new(0x456, 0x654));
        let (_, pair, tx) = cut.reception_parameters(extended(0x744)).unwrap();
        assert_eq!(tx, extended(0x523));
        assert_eq!(pair, TransportAddressPair::new(0x789, 0x321));
        assert!(cut.reception_parameters(0x2).is_none());
        assert!(cut.reception_parameters(can_id::INVALID_ID).is_none());
    }

    #[test]
    fn reception_address_is_matched() {
        let cut = NormalAddressingFilter::with_entries(&TEST_ENTRIES, &CODECS);
        assert!(!cut.matches(base(0x512)));
        assert!(cut.matches(base(0x513)));
        assert!(!cut.matches(base(0x514)));
        assert!(cut.matches(base(0x638)));
        assert!(cut.matches(base(0x745)));
        assert!(!cut.matches(base(0x746)));
        assert!(!cut.matches(extended(0x512)));
        assert!(cut.matches(extended(0x513)));
        assert!(!cut.matches(extended(0x514)));
        assert!(cut.matches(extended(0x638)));
        assert!(!cut.matches(extended(0x743)));
        assert!(cut.matches(extended(0x744)));
        assert!(!cut.matches(extended(0x745)));
        // The merger sees the base ids.
        let merged = BitFieldFilter::new();
        cut.accept_merger(&merged);
        assert!(merged.matches(base(0x745)));
        assert!(!merged.matches(base(0x746)));
    }

    #[test]
    fn table_with_invalid_entries_only() {
        static INVALID: [AddressEntry; 2] = [
            entry(can_id::INVALID_ID, base(0x123), 0x789, 0x111, 1, 2),
            entry(can_id::INVALID_ID, base(0x124), 0x222, 0x333, 1, 2),
        ];
        let cut = NormalAddressingFilter::with_entries(&INVALID, &CODECS);
        assert!(cut.transmission_parameters(&TransportAddressPair::new(0x457, 0x123)).is_none());
        assert!(cut.reception_parameters(can_id::INVALID_ID).is_none());
        let mut buffer = [0u8; 20];
        assert_eq!(cut.format_data_link_address(0x13f485, &mut buffer), b"0x0013f485");
        let mut short = [0x7fu8; 7];
        // The C++ string buffer keeps one byte for its terminator.
        assert_eq!(cut.format_data_link_address(0x13f485, &mut short[..6]), b"0x001");
        assert_eq!(short[6], 0x7f);
    }

    #[test]
    #[should_panic(expected = "monotonically increasing")]
    fn ascending_reception_addresses_expected() {
        static UNSORTED: [AddressEntry; 3] = [
            entry(base(0x513), base(0x638), 0xf54, 0x83, 1, 2),
            entry(base(0x745), base(0x52), 0x55, 0x999, 1, 2),
            entry(base(0x638), base(0x7ff), 0x1, 0xfe, 1, 2),
        ];
        let _ = NormalAddressingFilter::with_entries(&UNSORTED, &CODECS);
    }

    #[test]
    #[should_panic(expected = "must be valid")]
    fn only_invalid_addresses_expected_at_end() {
        static ENTRIES: [AddressEntry; 2] = [
            entry(can_id::INVALID_ID, base(0x123), 0x789, 0x111, 1, 2),
            entry(base(0x777), base(0x124), 0x222, 0x333, 1, 2),
        ];
        let _ = NormalAddressingFilter::with_entries(&ENTRIES, &CODECS);
    }

    #[test]
    fn entry_with_out_of_bounds_codec_indices() {
        static ENTRIES: [AddressEntry; 1] = [entry(base(0x123), base(0x456), 0x111, 0x222, 4, 4)];
        let cut = NormalAddressingFilter::with_entries(&ENTRIES, &CODECS);
        assert!(cut.transmission_parameters(&TransportAddressPair::new(0x222, 0x111)).is_none());
        assert!(cut.reception_parameters(base(0x123)).is_none());
        // A table slot that is `None` is no codec either.
        static NONE_ENTRIES: [AddressEntry; 1] =
            [entry(base(0x123), base(0x456), 0x111, 0x222, 0, 0)];
        let cut = NormalAddressingFilter::with_entries(&NONE_ENTRIES, &CODECS);
        assert!(cut.reception_parameters(base(0x123)).is_none());
    }
}
