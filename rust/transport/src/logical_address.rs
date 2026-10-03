// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Tester address lists, ported from `LogicalAddress.h` and `.cpp`: the 2-byte DoIP and
//! 1-byte CAN forms of a tester's address.

/// One tester, by its two addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LogicalAddress {
    /// The 2-byte address.
    pub address_2byte: u16,
    /// The 1-byte address.
    pub address_1byte: u16,
}

/// The entry of `list` with the 2-byte address `address`.
pub fn find_by_2byte_address(list: &[LogicalAddress], address: u16) -> Option<LogicalAddress> {
    list.iter().copied().find(|entry| entry.address_2byte == address)
}

/// The entry of `list` with the 1-byte address `address`.
pub fn find_by_1byte_address(list: &[LogicalAddress], address: u16) -> Option<LogicalAddress> {
    list.iter().copied().find(|entry| entry.address_1byte == address)
}

/// Whether `list` holds the 2-byte address `address`.
pub fn is_2byte_address_in(address: u16, list: &[LogicalAddress]) -> bool {
    find_by_2byte_address(list, address).is_some()
}

/// Whether `list` holds the 1-byte address `address`.
pub fn is_1byte_address_in(address: u16, list: &[LogicalAddress]) -> bool {
    find_by_1byte_address(list, address).is_some()
}

/// Converts between the two forms over several lists: the port of
/// `LogicalAddressConverter<N>`. An address in no list is returned unchanged.
pub struct LogicalAddressConverter {
    lists: &'static [&'static [LogicalAddress]],
}

impl LogicalAddressConverter {
    /// A converter over `lists`.
    pub const fn new(lists: &'static [&'static [LogicalAddress]]) -> Self {
        Self { lists }
    }

    /// The 1-byte form of `address`.
    pub fn convert_2byte_address_to_1byte(&self, address: u16) -> u16 {
        self.lists
            .iter()
            .find_map(|list| find_by_2byte_address(list, address))
            .map_or(address, |entry| entry.address_1byte)
    }

    /// The 2-byte form of `address`.
    pub fn convert_1byte_address_to_2byte(&self, address: u16) -> u16 {
        self.lists
            .iter()
            .find_map(|list| find_by_1byte_address(list, address))
            .map_or(address, |entry| entry.address_2byte)
    }
}

// Ported from transport/test/src/TesterAddressTest.cpp, with the reference application's
// tester lists.
#[cfg(test)]
mod tests {
    use super::*;

    const fn entry(address_2byte: u16, address_1byte: u16) -> LogicalAddress {
        LogicalAddress { address_2byte, address_1byte }
    }

    static ETHERNET: [LogicalAddress; 3] =
        [entry(0x0ECD, 0x00F0), entry(0x0ECE, 0x00F1), entry(0x0EFF, 0x00F2)];
    static CAN: [LogicalAddress; 2] = [entry(0x0E10, 0x00F3), entry(0x0E01, 0x00F4)];
    static CONVERTER: LogicalAddressConverter = LogicalAddressConverter::new(&[&ETHERNET, &CAN]);

    #[test]
    fn converter() {
        assert_eq!(CONVERTER.convert_2byte_address_to_1byte(0x0ECD), 0x00F0);
        assert_eq!(CONVERTER.convert_2byte_address_to_1byte(0x0E10), 0x00F3);
        assert_eq!(CONVERTER.convert_2byte_address_to_1byte(0x0E99), 0x0E99);
        assert_eq!(CONVERTER.convert_1byte_address_to_2byte(0x00F4), 0x0E01);
        assert_eq!(CONVERTER.convert_1byte_address_to_2byte(0x00F2), 0x0EFF);
        assert_eq!(CONVERTER.convert_1byte_address_to_2byte(0x00FE), 0x00FE);
    }

    #[test]
    fn find() {
        assert!(is_2byte_address_in(0x0ECD, &ETHERNET));
        assert!(is_2byte_address_in(0x0E01, &CAN));
        assert!(!is_2byte_address_in(0x0E01, &ETHERNET));
        assert!(!is_2byte_address_in(0x0ECD, &CAN));
        assert!(is_1byte_address_in(0x00F1, &ETHERNET));
        assert!(is_1byte_address_in(0x00F4, &CAN));
        assert!(!is_1byte_address_in(0x00F4, &ETHERNET));
        assert!(!is_1byte_address_in(0x00F1, &CAN));
    }
}
