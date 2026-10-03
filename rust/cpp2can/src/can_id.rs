// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The id encoding, ported from `canframes/CanId.h`: a 32-bit value holding the raw
//! 11- or 29-bit identifier plus qualifier bits.

/// Set in an id that holds a 29-bit extended identifier.
pub const EXTENDED_QUALIFIER_BIT: u32 = 0x8000_0000;
/// Set in an id that is not valid.
pub const INVALID_QUALIFIER_BIT: u32 = 0x4000_0000;
/// Set in an id whose frame must not be sent as CAN FD.
pub const FORCE_NON_FD_QUALIFIER_BIT: u32 = 0x2000_0000;
/// The invalid id (`CanId::Invalid::value`).
pub const INVALID_ID: u32 = 0xffff_ffff;
/// The width of a base identifier.
pub const BASE_ID_BITS: u32 = 11;
/// The largest raw base identifier.
pub const MAX_RAW_BASE_ID: u32 = (1 << BASE_ID_BITS) - 1;
/// The largest raw extended identifier.
pub const MAX_RAW_EXTENDED_ID: u32 = 0x1fff_ffff;

/// The id of a base identifier (`CanId::base`, `CanId::Base<RawId>::value`).
pub const fn base(base_id: u16) -> u32 {
    base_id as u32
}

/// The id of an extended identifier (`CanId::extended`, `CanId::Extended<RawId>::value`).
pub const fn extended(extended_id: u32) -> u32 {
    extended_id | EXTENDED_QUALIFIER_BIT
}

/// `id` marked as not to be sent as CAN FD.
pub const fn force_no_fd(id: u32) -> u32 {
    id | FORCE_NON_FD_QUALIFIER_BIT
}

/// The id of raw identifier `value`, extended or base (`CanId::Id<RawId, IsExtended>`).
pub const fn id(value: u32, is_value_extended: bool) -> u32 {
    value | if is_value_extended { EXTENDED_QUALIFIER_BIT } else { 0 }
}

/// The id of raw identifier `value`, extended or base, optionally forced to classic CAN.
pub const fn id_with_fd(value: u32, is_value_extended: bool, force_no_fd: bool) -> u32 {
    id(value, is_value_extended) | if force_no_fd { FORCE_NON_FD_QUALIFIER_BIT } else { 0 }
}

/// The raw identifier of `value`, without its qualifier bits.
pub const fn raw_id(value: u32) -> u32 {
    value & !(EXTENDED_QUALIFIER_BIT | FORCE_NON_FD_QUALIFIER_BIT)
}

/// Whether `value` is a valid id.
pub const fn is_valid(value: u32) -> bool {
    (value & INVALID_QUALIFIER_BIT) == 0
}

/// Whether `value` holds a base identifier.
pub const fn is_base(value: u32) -> bool {
    (value & EXTENDED_QUALIFIER_BIT) == 0
}

/// Whether `value` holds an extended identifier.
pub const fn is_extended(value: u32) -> bool {
    (value & EXTENDED_QUALIFIER_BIT) != 0
}

/// Whether `value` must not be sent as CAN FD.
pub const fn is_force_no_fd(value: u32) -> bool {
    (value & FORCE_NON_FD_QUALIFIER_BIT) != 0
}

// Ported from cpp2can/test/src/can/canframes/CanIdTest.cpp.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_functions() {
        assert_eq!(base(0x7ff), 0x7ff);
        assert_eq!(extended(0x789ab), 0x800789ab);
        assert_eq!(id(0x789ab, true), 0x800789ab);
        assert_eq!(id(0x789ab, false), 0x000789ab);
        assert_eq!(id(0x7ff, false), 0x7ff);
        assert_eq!(id(0x7ff, true), 0x800007ff);
        assert_eq!(id_with_fd(0x7ff, false, true), 0x200007ff);
        assert_eq!(id_with_fd(0x7ff, true, true), 0xA00007FF);
        assert_eq!(id_with_fd(0x7ff, false, false), 0x000007ff);
        assert_eq!(force_no_fd(0x7ff), 0x200007ff);
        assert_eq!(raw_id(0xA00007FF), 0x7ff);
    }

    #[test]
    fn type_based_value_retrieval() {
        const BASE: u32 = base(0x7ff);
        const EXTENDED: u32 = extended(0x789ab);
        const ID_EXTENDED: u32 = id(0x789ab, true);
        const ID_BASE: u32 = id(0x7ff, false);
        assert_eq!(BASE, 0x7ff);
        assert_eq!(EXTENDED, 0x800789ab);
        assert_eq!(ID_EXTENDED, 0x800789ab);
        assert_eq!(ID_BASE, 0x7ff);
        assert_eq!(INVALID_ID, 0xffffffff);
    }

    #[test]
    fn checks() {
        assert!(is_force_no_fd(0x200007ff));
        assert!(!is_force_no_fd(0x000007ff));
    }

    #[test]
    fn value_and_checks() {
        assert_eq!(raw_id(extended(0x78b)), 0x78b);
        assert_eq!(raw_id(base(0x78b)), 0x78b);
        assert!(is_valid(base(0x07ff)));
        assert!(!is_valid(base(0x7ff) | 0x40000000));
        assert!(is_valid(extended(0x1fffffff)));
        assert!(!is_valid(extended(0x1fffffff) | 0x40000000));
        assert!(!is_valid(INVALID_ID));
        assert!(is_extended(extended(0x789)));
        assert!(!is_base(extended(0x789)));
        assert!(!is_extended(base(0x789)));
        assert!(is_base(base(0x789)));
    }
}
