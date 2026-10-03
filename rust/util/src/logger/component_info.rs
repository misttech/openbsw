// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Component names, ported from `util/logger/ComponentInfo`.

use crate::format::{AttributedString, StringAttributes};

/// The index of no component.
pub const COMPONENT_NONE: u8 = 0xff;

/// Constant information about a component: its attributed name.
#[derive(Debug)]
pub struct PlainComponentInfo {
    name: AttributedString<'static>,
}

impl PlainComponentInfo {
    /// Information naming a component.
    pub const fn new(name: &'static [u8], attributes: StringAttributes) -> Self {
        Self { name: AttributedString::new(name, attributes) }
    }

    /// The attributed name.
    pub const fn name(&self) -> AttributedString<'static> {
        self.name
    }
}

/// A component's index and a reference to its information, which may be invalid.
#[derive(Clone, Copy, Debug)]
pub struct ComponentInfo {
    component_index: u8,
    plain: Option<&'static PlainComponentInfo>,
}

impl ComponentInfo {
    /// Information about the component `component_index`; invalid when `plain` is `None`.
    pub const fn new(component_index: u8, plain: Option<&'static PlainComponentInfo>) -> Self {
        Self { component_index, plain }
    }

    /// Whether the information is valid.
    pub fn is_valid(&self) -> bool {
        self.plain.is_some()
    }

    /// The component's index.
    pub fn index(&self) -> u8 {
        self.component_index
    }

    /// The attributed name. Panics when invalid, as the C++ assertion does.
    pub fn name(&self) -> AttributedString<'static> {
        self.plain.expect("info must not be null").name
    }

    /// The plain name. Panics when invalid.
    pub fn plain_info_string(&self) -> &'static [u8] {
        self.plain.expect("info must not be null").name.string()
    }
}

impl Default for ComponentInfo {
    fn default() -> Self {
        Self::new(COMPONENT_NONE, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{BOLD, Color};

    static PLAIN_INFO: PlainComponentInfo =
        PlainComponentInfo::new(b"abc", StringAttributes::new(Color::Yellow, BOLD, Color::Black));

    // Ported from util/test/src/util/logger/ComponentInfoTest.cpp.
    #[test]
    fn component_info() {
        assert!(!ComponentInfo::default().is_valid());
        let cut = ComponentInfo::new(12, Some(&PLAIN_INFO));
        assert!(cut.is_valid());
        assert_eq!(cut.index(), 12);
        assert_eq!(cut.name().string(), b"abc");
        assert_eq!(
            *cut.name().attributes(),
            StringAttributes::new(Color::Yellow, BOLD, Color::Black)
        );
        let copy = cut;
        assert_eq!(copy.index(), 12);
        assert_eq!(cut.plain_info_string(), b"abc");
    }

    #[test]
    #[should_panic(expected = "info must not be null")]
    fn invalid_name_panics() {
        let _ = ComponentInfo::new(12, None).name();
    }

    #[test]
    #[should_panic(expected = "info must not be null")]
    fn invalid_plain_string_panics() {
        let _ = ComponentInfo::new(12, None).plain_info_string();
    }
}
