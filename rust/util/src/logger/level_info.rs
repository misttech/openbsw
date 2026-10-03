// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Log levels and their display, ported from `util/logger/LevelInfo`.

use crate::format::{AttributedString, BOLD, Color, StringAttributes};

/// The severity of a log message. `None` only filters out all messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Level {
    /// Debug.
    Debug = 0,
    /// Info.
    Info = 1,
    /// Warning.
    Warn = 2,
    /// Error.
    Error = 3,
    /// Critical.
    Critical = 4,
    /// No logging.
    None = 5,
}

/// The number of levels, `None` included.
pub const LEVEL_COUNT: usize = 6;

impl Level {
    /// The level with discriminant `value`, or `None` for an unknown one.
    pub const fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Debug,
            1 => Self::Info,
            2 => Self::Warn,
            3 => Self::Error,
            4 => Self::Critical,
            _ => Self::None,
        }
    }
}

/// Constant information about a level: its attributed name and value.
#[derive(Debug)]
pub struct PlainLevelInfo {
    name: AttributedString<'static>,
    level: Level,
}

impl PlainLevelInfo {
    /// Information naming `level`.
    pub const fn new(name: &'static [u8], attributes: StringAttributes, level: Level) -> Self {
        Self { name: AttributedString::new(name, attributes), level }
    }

    /// The attributed name.
    pub const fn name(&self) -> AttributedString<'static> {
        self.name
    }

    /// The level.
    pub const fn level(&self) -> Level {
        self.level
    }
}

/// A reference to a level's information, which may be invalid.
#[derive(Clone, Copy, Debug)]
pub struct LevelInfo {
    plain: Option<&'static PlainLevelInfo>,
}

const PLAIN: StringAttributes = StringAttributes::color(Color::DefaultColor);

static DEFAULT_LEVEL_INFOS: [PlainLevelInfo; LEVEL_COUNT] = [
    PlainLevelInfo::new(b"DEBUG", PLAIN, Level::Debug),
    PlainLevelInfo::new(b"INFO", PLAIN, Level::Info),
    PlainLevelInfo::new(
        b"WARN",
        StringAttributes::new(Color::Yellow, BOLD, Color::DefaultColor),
        Level::Warn,
    ),
    PlainLevelInfo::new(
        b"ERROR",
        StringAttributes::new(Color::Red, BOLD, Color::DefaultColor),
        Level::Error,
    ),
    PlainLevelInfo::new(b"CRITICAL", PLAIN, Level::Critical),
    PlainLevelInfo::new(b"NONE", PLAIN, Level::None),
];

impl LevelInfo {
    /// The default table of level infos, indexed by level.
    pub const fn default_table() -> &'static [PlainLevelInfo; LEVEL_COUNT] {
        &DEFAULT_LEVEL_INFOS
    }

    /// Information referring to `plain`; invalid when `None`.
    pub const fn new(plain: Option<&'static PlainLevelInfo>) -> Self {
        Self { plain }
    }

    /// Whether the information is valid.
    pub fn is_valid(&self) -> bool {
        self.plain.is_some()
    }

    /// The attributed name. Panics when invalid, as the C++ assertion does.
    pub fn name(&self) -> AttributedString<'static> {
        self.plain.expect("info must not be null").name
    }

    /// The level. Panics when invalid.
    pub fn level(&self) -> Level {
        self.plain.expect("info must not be null").level
    }

    /// The plain name. Panics when invalid.
    pub fn plain_info_string(&self) -> &'static [u8] {
        self.plain.expect("info must not be null").name.string()
    }
}

impl Default for LevelInfo {
    fn default() -> Self {
        Self::new(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static PLAIN_INFO: PlainLevelInfo = PlainLevelInfo::new(
        b"abc",
        StringAttributes::new(Color::Yellow, BOLD, Color::Black),
        Level::Debug,
    );

    // Ported from util/test/src/util/logger/LevelInfoTest.cpp.
    #[test]
    fn level_info() {
        assert!(!LevelInfo::default().is_valid());
        let cut = LevelInfo::new(Some(&PLAIN_INFO));
        assert!(cut.is_valid());
        assert_eq!(cut.level(), Level::Debug);
        assert_eq!(cut.name().string(), b"abc");
        assert_eq!(
            *cut.name().attributes(),
            StringAttributes::new(Color::Yellow, BOLD, Color::Black)
        );
        let copy = cut;
        assert_eq!(copy.level(), Level::Debug);
        assert_eq!(cut.plain_info_string(), b"abc");
    }

    #[test]
    #[should_panic(expected = "info must not be null")]
    fn invalid_name_panics() {
        let _ = LevelInfo::new(None).name();
    }

    #[test]
    #[should_panic(expected = "info must not be null")]
    fn invalid_level_panics() {
        let _ = LevelInfo::new(None).level();
    }

    #[test]
    fn default_table_names_and_styles() {
        let table = LevelInfo::default_table();
        assert_eq!(table[Level::Debug as usize].name.string(), b"DEBUG");
        assert_eq!(table[Level::Warn as usize].name.attributes().foreground_color(), Color::Yellow);
        assert_eq!(table[Level::Error as usize].name.attributes().format(), BOLD);
        assert_eq!(table[Level::None as usize].level, Level::None);
        assert_eq!(Level::from_u8(3), Level::Error);
        assert_eq!(Level::from_u8(9), Level::None);
    }
}
