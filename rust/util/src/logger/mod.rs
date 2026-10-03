// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The logger facade, ported from `util/logger`.
//!
//! [`Logger`] separates emitting log messages from handling them: a [`ComponentMapping`]
//! decides whether a message is accepted and names its component and level, and a
//! [`LoggerOutput`] handles the accepted ones. Until the application initializes the
//! facade, every message is dropped.
//!
//! Components are declared as [`LoggerComponent`] statics (the port of
//! `DEFINE_LOGGER_COMPONENT`); the mapping assigns their indices at startup.

mod component_info;
mod level_info;

pub use component_info::{COMPONENT_NONE, ComponentInfo, PlainComponentInfo};
pub use level_info::{LEVEL_COUNT, Level, LevelInfo, PlainLevelInfo};

use core::sync::atomic::{AtomicU8, Ordering};

use crate::cell::RacyCell;
use crate::format::Arg;

/// Maps a component index to its level filter, name and level infos: the port of
/// `IComponentMapping`.
pub trait ComponentMapping: Sync {
    /// Whether logs of `level` or higher are enabled for the component.
    fn is_enabled(&self, component_index: u8, level: Level) -> bool;

    /// The minimum level for which logs are enabled for the component.
    fn level(&self, component_index: u8) -> Level;

    /// Human-readable information about a level.
    fn level_info(&self, level: Level) -> LevelInfo;

    /// Human-readable information about a component; invalid for an unknown index.
    fn component_info(&self, component_index: u8) -> ComponentInfo;
}

/// Receives filtered log messages: the port of `ILoggerOutput`.
pub trait LoggerOutput: Sync {
    /// Handle one message: a printf-style `format` with its `args`.
    fn log_output(
        &self,
        component_info: &ComponentInfo,
        level_info: &LevelInfo,
        format: &[u8],
        args: &[Arg<'_>],
    );
}

/// A logger component: its index is assigned by the mapping at startup, before which it
/// is [`COMPONENT_NONE`].
pub struct LoggerComponent {
    index: AtomicU8,
}

impl LoggerComponent {
    /// An unassigned component.
    pub const fn new() -> Self {
        Self { index: AtomicU8::new(COMPONENT_NONE) }
    }

    /// The component's index.
    pub fn index(&self) -> u8 {
        self.index.load(Ordering::Relaxed)
    }

    /// Assign the component's index.
    pub fn set_index(&self, index: u8) {
        self.index.store(index, Ordering::Relaxed);
    }
}

impl Default for LoggerComponent {
    fn default() -> Self {
        Self::new()
    }
}

struct Binding {
    mapping: &'static dyn ComponentMapping,
    output: &'static dyn LoggerOutput,
}

static BINDING: RacyCell<Option<Binding>> = RacyCell::new(None);

/// The static logging facade.
pub struct Logger;

impl Logger {
    /// Connect the facade to `mapping` and `output`. Call during startup, before the
    /// tasks that log run.
    pub fn init(mapping: &'static dyn ComponentMapping, output: &'static dyn LoggerOutput) {
        // SAFETY: called during single-threaded startup, before any `log` runs.
        unsafe { BINDING.set(Some(Binding { mapping, output })) };
    }

    /// Cut the connection; later messages are dropped.
    pub fn shutdown() {
        // SAFETY: the C++ facade documents that lower-priority tasks may still log after
        // shutdown; the application calls this when no task logs anymore.
        unsafe { BINDING.set(None) };
    }

    fn binding() -> Option<&'static Binding> {
        // SAFETY: `init` and `shutdown` run while no other task uses the facade.
        unsafe { BINDING.get() }.as_ref()
    }

    /// Whether logs of `level` or higher are enabled for the component.
    pub fn is_enabled(component_index: u8, level: Level) -> bool {
        Self::binding().is_some_and(|binding| binding.mapping.is_enabled(component_index, level))
    }

    /// The minimum level enabled for the component, `None` when uninitialized.
    pub fn level(component_index: u8) -> Level {
        Self::binding().map_or(Level::None, |binding| binding.mapping.level(component_index))
    }

    /// Emit a message of `level` for the component.
    pub fn log(component_index: u8, level: Level, format: &[u8], args: &[Arg<'_>]) {
        if let Some(binding) = Self::binding()
            && binding.mapping.is_enabled(component_index, level)
        {
            let component_info = binding.mapping.component_info(component_index);
            let level_info = binding.mapping.level_info(level);
            binding.output.log_output(&component_info, &level_info, format, args);
        }
    }

    /// Emit a `Debug` message.
    pub fn debug(component_index: u8, format: &[u8], args: &[Arg<'_>]) {
        Self::log(component_index, Level::Debug, format, args);
    }

    /// Emit an `Info` message.
    pub fn info(component_index: u8, format: &[u8], args: &[Arg<'_>]) {
        Self::log(component_index, Level::Info, format, args);
    }

    /// Emit a `Warn` message.
    pub fn warn(component_index: u8, format: &[u8], args: &[Arg<'_>]) {
        Self::log(component_index, Level::Warn, format, args);
    }

    /// Emit an `Error` message.
    pub fn error(component_index: u8, format: &[u8], args: &[Arg<'_>]) {
        Self::log(component_index, Level::Error, format, args);
    }

    /// Emit a `Critical` message.
    pub fn critical(component_index: u8, format: &[u8], args: &[Arg<'_>]) {
        Self::log(component_index, Level::Critical, format, args);
    }
}

/// Emit a message for a [`LoggerComponent`] at a level:
/// `log!(DEMO, Level::Debug, b"Sending frame %d", count)`.
#[macro_export]
macro_rules! log {
    ($component:expr, $level:expr, $format:expr $(, $arg:expr)* $(,)?) => {
        $crate::logger::Logger::log($component.index(), $level, $format, &[$($crate::format::Arg::from($arg)),*])
    };
}

/// Emit a `Debug` message for a [`LoggerComponent`].
#[macro_export]
macro_rules! log_debug {
    ($component:expr, $format:expr $(, $arg:expr)* $(,)?) => {
        $crate::log!($component, $crate::logger::Level::Debug, $format $(, $arg)*)
    };
}

/// Emit an `Info` message for a [`LoggerComponent`].
#[macro_export]
macro_rules! log_info {
    ($component:expr, $format:expr $(, $arg:expr)* $(,)?) => {
        $crate::log!($component, $crate::logger::Level::Info, $format $(, $arg)*)
    };
}

/// Emit a `Warn` message for a [`LoggerComponent`].
#[macro_export]
macro_rules! log_warn {
    ($component:expr, $format:expr $(, $arg:expr)* $(,)?) => {
        $crate::log!($component, $crate::logger::Level::Warn, $format $(, $arg)*)
    };
}

/// Emit an `Error` message for a [`LoggerComponent`].
#[macro_export]
macro_rules! log_error {
    ($component:expr, $format:expr $(, $arg:expr)* $(,)?) => {
        $crate::log!($component, $crate::logger::Level::Error, $format $(, $arg)*)
    };
}

/// Emit a `Critical` message for a [`LoggerComponent`].
#[macro_export]
macro_rules! log_critical {
    ($component:expr, $format:expr $(, $arg:expr)* $(,)?) => {
        $crate::log!($component, $crate::logger::Level::Critical, $format $(, $arg)*)
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{Color, PrintfFormatter, StringAttributes};
    use crate::stream::StringBufferOutputStream;
    use core::cell::{Cell, RefCell};
    use std::string::String;
    use std::sync::Mutex;

    /// The tests share the global facade, so they run one at a time.
    static SERIAL: Mutex<()> = Mutex::new(());

    struct Fixture {
        enabled: Cell<bool>,
        level: Cell<Level>,
        component_index: Cell<u8>,
        component_info: Cell<ComponentInfo>,
        level_info: Cell<LevelInfo>,
        out: RefCell<Option<(u8, Level, String)>>,
    }

    // SAFETY: the tests run serialized under `SERIAL`, so the cells are never shared
    // between threads at the same time.
    unsafe impl Sync for Fixture {}

    impl ComponentMapping for Fixture {
        fn is_enabled(&self, component_index: u8, level: Level) -> bool {
            self.component_index.set(component_index);
            self.level.set(level);
            self.enabled.get()
        }

        fn level(&self, component_index: u8) -> Level {
            self.component_index.set(component_index);
            self.level.get()
        }

        fn level_info(&self, level: Level) -> LevelInfo {
            self.level.set(level);
            self.level_info.get()
        }

        fn component_info(&self, component_index: u8) -> ComponentInfo {
            self.component_index.set(component_index);
            self.component_info.get()
        }
    }

    impl LoggerOutput for Fixture {
        fn log_output(
            &self,
            component_info: &ComponentInfo,
            level_info: &LevelInfo,
            format: &[u8],
            args: &[Arg<'_>],
        ) {
            let mut buffer = [0u8; 300];
            let mut stream = StringBufferOutputStream::new(&mut buffer, b"", b"");
            PrintfFormatter::new(&mut stream, true).format_args(format, args);
            let text = String::from_utf8_lossy(stream.string()).into_owned();
            *self.out.borrow_mut() = Some((component_info.index(), level_info.level(), text));
        }
    }

    static PLAIN_COMPONENT: PlainComponentInfo =
        PlainComponentInfo::new(b"abc", StringAttributes::color(Color::DefaultColor));

    static FIXTURE: Fixture = Fixture {
        enabled: Cell::new(false),
        level: Cell::new(Level::None),
        component_index: Cell::new(COMPONENT_NONE),
        component_info: Cell::new(ComponentInfo::new(12, Some(&PLAIN_COMPONENT))),
        level_info: Cell::new(LevelInfo::new(Some(
            &LevelInfo::default_table()[Level::Debug as usize],
        ))),
        out: RefCell::new(None),
    };

    fn check_and_reset_log(
        component_index: u8,
        level: Level,
        out_component_index: u8,
        out_level: Level,
        text: &str,
    ) -> bool {
        let out = FIXTURE.out.borrow_mut().take();
        let result = FIXTURE.component_index.get() == component_index
            && FIXTURE.level.get() == level
            && out.as_ref().is_some_and(|(index, level, message)| {
                *index == out_component_index && *level == out_level && message == text
            });
        FIXTURE.component_index.set(COMPONENT_NONE);
        FIXTURE.level.set(Level::None);
        result
    }

    // Ported from util/test/src/util/logger/LoggerTest.cpp.
    #[test]
    fn enabled() {
        let _guard = SERIAL.lock().unwrap();
        Logger::init(&FIXTURE, &FIXTURE);
        FIXTURE.enabled.set(true);
        assert!(Logger::is_enabled(15, Level::Debug));
        assert_eq!(FIXTURE.component_index.get(), 15);
        assert_eq!(FIXTURE.level.get(), Level::Debug);
        FIXTURE.enabled.set(false);
        assert!(!Logger::is_enabled(16, Level::Info));
        assert_eq!(FIXTURE.component_index.get(), 16);
        FIXTURE.enabled.set(true);
        assert!(Logger::is_enabled(17, Level::Error));
        Logger::shutdown();
        assert!(!Logger::is_enabled(18, Level::Info));
        assert_eq!(FIXTURE.component_index.get(), 17);
        assert_eq!(FIXTURE.level.get(), Level::Error);
    }

    #[test]
    fn level() {
        let _guard = SERIAL.lock().unwrap();
        Logger::init(&FIXTURE, &FIXTURE);
        FIXTURE.level.set(Level::Debug);
        assert_eq!(Logger::level(10), Level::Debug);
        assert_eq!(FIXTURE.component_index.get(), 10);
        FIXTURE.level.set(Level::None);
        assert_eq!(Logger::level(11), Level::None);
        Logger::shutdown();
    }

    #[test]
    fn logging() {
        let _guard = SERIAL.lock().unwrap();
        Logger::init(&FIXTURE, &FIXTURE);
        FIXTURE.enabled.set(true);
        Logger::log(1, Level::Info, b"abc: %d %s", &[12_i32.into(), "log".into()]);
        assert!(check_and_reset_log(1, Level::Info, 12, Level::Debug, "abc: 12 log"));
        Logger::debug(2, b"abc: %d %s", &[13_i32.into(), "debug".into()]);
        assert!(check_and_reset_log(2, Level::Debug, 12, Level::Debug, "abc: 13 debug"));
        Logger::info(3, b"abc: %d %s", &[14_i32.into(), "info".into()]);
        assert!(check_and_reset_log(3, Level::Info, 12, Level::Debug, "abc: 14 info"));
        Logger::warn(4, b"abc: %d %s", &[14_i32.into(), "warn".into()]);
        assert!(check_and_reset_log(4, Level::Warn, 12, Level::Debug, "abc: 14 warn"));
        Logger::error(5, b"abc: %d %s", &[15_i32.into(), "error".into()]);
        assert!(check_and_reset_log(5, Level::Error, 12, Level::Debug, "abc: 15 error"));
        Logger::critical(6, b"abc: %d %s", &[16_i32.into(), "critical".into()]);
        assert!(check_and_reset_log(6, Level::Critical, 12, Level::Debug, "abc: 16 critical"));
        static DEMO: LoggerComponent = LoggerComponent::new();
        DEMO.set_index(7);
        crate::log_info!(DEMO, b"abc: %d %s", 17_i32, "vlog");
        assert!(check_and_reset_log(7, Level::Info, 12, Level::Debug, "abc: 17 vlog"));
        Logger::shutdown();
    }

    #[test]
    fn uninitialized_usage() {
        let _guard = SERIAL.lock().unwrap();
        Logger::shutdown();
        assert!(!Logger::is_enabled(1, Level::Info));
        Logger::log(0, Level::Debug, b"abc", &[1_i32.into(), 2_i32.into(), 3_i32.into()]);
        Logger::info(1, b"abc", &[]);
        Logger::debug(2, b"abc", &[]);
        Logger::warn(3, b"abc", &[]);
        Logger::error(4, b"abc", &[]);
        Logger::critical(5, b"abc", &[]);
        assert_eq!(Logger::level(0), Level::None);
        assert!(FIXTURE.out.borrow().is_none());
    }

    #[test]
    fn components_start_unassigned() {
        let component = LoggerComponent::new();
        assert_eq!(component.index(), COMPONENT_NONE);
        component.set_index(3);
        assert_eq!(component.index(), 3);
    }
}
