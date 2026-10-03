// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Byte-string helpers, ported from `util/string/ConstString.h`.
//!
//! `ConstString` is a `(pointer, length)` view with comparison operations. A `&[u8]`
//! is the same thing in Rust, so this module only provides the operations that `[u8]`
//! lacks: the C-style three-way comparisons and `find`.

/// Convert an ASCII upper-case letter to lower case; other bytes are unchanged.
fn to_lower(c: u8) -> i32 {
    if c.is_ascii_uppercase() {
        i32::from(c) + i32::from(b'a') - i32::from(b'A')
    } else {
        i32::from(c)
    }
}

/// Compare two strings as `ConstString::compare` does.
///
/// Returns a value less than 0 if the first byte that does not match has a lower value
/// in `lhs` than in `rhs`, 0 if both strings are equal, and a value greater than 0 if it
/// has a higher value in `lhs`. When one string is a prefix of the other, the result is
/// the difference of the lengths.
pub fn compare(lhs: &[u8], rhs: &[u8]) -> i32 {
    let compare_length = lhs.len().min(rhs.len());
    let mut result = 0;
    let mut i = 0;
    while result == 0 && i < compare_length {
        result = i32::from(lhs[i]) - i32::from(rhs[i]);
        i += 1;
    }
    if result == 0 {
        result = lhs.len() as i32 - rhs.len() as i32;
    }
    result
}

/// Compare two strings ignoring ASCII case, as `ConstString::compareIgnoreCase` does.
pub fn compare_ignore_case(lhs: &[u8], rhs: &[u8]) -> i32 {
    let compare_length = lhs.len().min(rhs.len());
    let mut result = 0;
    let mut i = 0;
    while result == 0 && i < compare_length {
        result = to_lower(lhs[i]) - to_lower(rhs[i]);
        i += 1;
    }
    if result == 0 {
        result = lhs.len() as i32 - rhs.len() as i32;
    }
    result
}

/// Find the first occurrence of `needle` in `haystack` at or after `offset`.
///
/// Returns the offset from the start of `haystack`, or a negative value when the string
/// is not found or `offset` is past the end.
pub fn find(haystack: &[u8], needle: &[u8], offset: usize) -> i32 {
    const NOT_FOUND: i32 = -1;
    if offset > haystack.len() {
        return NOT_FOUND;
    }
    if needle.is_empty() {
        return offset as i32;
    }
    let rest = &haystack[offset..];
    rest.windows(needle.len())
        .position(|window| window == needle)
        .map_or(NOT_FOUND, |pos| (pos + offset) as i32)
}

/// Whether `haystack` contains `needle`.
pub fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    find(haystack, needle, 0) >= 0
}

/// The bytes of a NUL-terminated byte string, up to but excluding the first NUL.
///
/// Rust string literals carry no terminator, so this only matters for buffers that a
/// C-style writer filled.
pub fn c_str(bytes: &[u8]) -> &[u8] {
    match bytes.iter().position(|&b| b == 0) {
        Some(len) => &bytes[..len],
        None => bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ported from util/test/src/util/string/ConstStringTest.cpp, the cases that map onto
    // slices.
    #[test]
    fn compares_like_strcmp_with_length_tie_break() {
        assert_eq!(compare(b"abc", b"abc"), 0);
        assert!(compare(b"abc", b"abd") < 0);
        assert!(compare(b"abd", b"abc") > 0);
        assert!(compare(b"ab", b"abc") < 0);
        assert!(compare(b"abc", b"ab") > 0);
        assert_eq!(compare(b"", b""), 0);
    }

    #[test]
    fn compares_ignoring_ascii_case() {
        assert_eq!(compare_ignore_case(b"aBc", b"AbC"), 0);
        assert!(compare_ignore_case(b"abc", b"ABD") < 0);
        assert!(compare_ignore_case(b"test", b"abc") > 0);
        assert!(compare_ignore_case(b"test", b"tests") < 0);
    }

    #[test]
    fn finds_substrings() {
        assert_eq!(find(b"abcdef", b"cd", 0), 2);
        assert_eq!(find(b"abcdef", b"cd", 3), -1);
        assert_eq!(find(b"abcabc", b"abc", 1), 3);
        assert_eq!(find(b"abc", b"abcd", 0), -1);
        assert_eq!(find(b"abc", b"", 7), -1);
        assert!(contains(b"abcdef", b"def"));
        assert!(!contains(b"abcdef", b"xyz"));
    }

    #[test]
    fn c_str_stops_at_nul() {
        assert_eq!(c_str(b"abc\0def"), b"abc");
        assert_eq!(c_str(b"abc"), b"abc");
        assert_eq!(c_str(b""), b"");
    }
}
