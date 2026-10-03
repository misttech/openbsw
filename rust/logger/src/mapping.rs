// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! The component table, ported from `logger/ComponentMapping.h`.

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use openbsw_util::format::StringAttributes;
use openbsw_util::logger::{
    COMPONENT_NONE, ComponentInfo, ComponentMapping as ComponentMappingTrait, LEVEL_COUNT, Level,
    LevelInfo, LoggerComponent, PlainComponentInfo, PlainLevelInfo,
};
use openbsw_util::string::compare_ignore_case;

/// One row of the table: the port of `PlainLoggerMappingInfo`, as the
/// `LOGGER_COMPONENT_MAPPING_INFO(level, NAME, color)` macro fills it.
pub struct MappingInfo {
    component: &'static LoggerComponent,
    info: PlainComponentInfo,
    initial_level: Level,
}

impl MappingInfo {
    /// A row naming `component` `name`, shown with `attributes`, enabled from
    /// `initial_level`.
    pub const fn new(
        initial_level: Level,
        component: &'static LoggerComponent,
        name: &'static [u8],
        attributes: StringAttributes,
    ) -> Self {
        Self { component, info: PlainComponentInfo::new(name, attributes), initial_level }
    }
}

/// The component table with each component's level: the port of
/// `ComponentMapping<IndexUpperBound>`.
///
/// Levels are atomics because the console's logger command changes them at run time.
pub struct ComponentMapping<const N: usize> {
    infos: &'static [MappingInfo; N],
    levels: [AtomicU8; N],
    global: Option<&'static LoggerComponent>,
    global_index: AtomicU8,
    global_level: AtomicU8,
    global_resolved: AtomicBool,
    level_infos: &'static [PlainLevelInfo],
}

impl<const N: usize> ComponentMapping<N> {
    /// The number of components.
    pub const MAPPING_SIZE: u8 = N as u8;

    /// A mapping over `infos` with `level_infos` naming the levels; `global` is the
    /// component whose level gates every other one (none if it is not in the table).
    ///
    /// C++ finds the global component in its constructor; a `const fn` cannot compare
    /// addresses, so the mapping finds it on first use, before any level is read or set,
    /// which gives the same answers: the global component's initial level gates every
    /// message from the start, as in C++, not only once
    /// [`apply_mapping`](Self::apply_mapping) has run.
    pub const fn new(
        infos: &'static [MappingInfo; N],
        level_infos: &'static [PlainLevelInfo],
        global: Option<&'static LoggerComponent>,
    ) -> Self {
        let mut levels = [const { AtomicU8::new(0) }; N];
        let mut idx = 0;
        while idx < N {
            levels[idx] = AtomicU8::new(infos[idx].initial_level as u8);
            idx += 1;
        }
        Self {
            infos,
            levels,
            global,
            global_index: AtomicU8::new(COMPONENT_NONE),
            global_level: AtomicU8::new(Level::Debug as u8),
            global_resolved: AtomicBool::new(false),
            level_infos,
        }
    }

    /// Assign every component its index.
    pub fn apply_mapping(&self) {
        for (idx, info) in self.infos.iter().enumerate() {
            info.component.set_index(idx as u8);
        }
    }

    /// Find the global component and take its level, once: the C++ constructor's loop.
    fn resolve_global(&self) {
        if self.global_resolved.load(Ordering::Relaxed) {
            return;
        }
        for (idx, info) in self.infos.iter().enumerate() {
            if self.global.is_some_and(|global| core::ptr::eq(global, info.component)) {
                self.global_index.store(idx as u8, Ordering::Relaxed);
                self.global_level
                    .store(self.levels[idx].load(Ordering::Relaxed), Ordering::Relaxed);
            }
        }
        self.global_resolved.store(true, Ordering::Relaxed);
    }

    /// Unassign every component.
    pub fn clear_mapping(&self) {
        for info in self.infos.iter() {
            info.component.set_index(COMPONENT_NONE);
        }
    }

    /// The number of components.
    pub fn mapping_size(&self) -> u8 {
        Self::MAPPING_SIZE
    }

    /// Set a component's level; an unknown component or level changes nothing.
    pub fn set_level(&self, component_index: u8, level: Level) {
        self.resolve_global();
        if usize::from(component_index) < N && (level as usize) < LEVEL_COUNT {
            self.levels[usize::from(component_index)].store(level as u8, Ordering::Relaxed);
            if component_index == self.global_index.load(Ordering::Relaxed) {
                self.global_level.store(level as u8, Ordering::Relaxed);
            }
        }
    }

    /// The level named `level_name`, ignoring case; invalid if none.
    pub fn level_info_by_name(&self, level_name: &[u8]) -> LevelInfo {
        for info in self.level_infos.iter().take(LEVEL_COUNT) {
            if compare_ignore_case(level_name, info.name().string()) == 0 {
                return LevelInfo::new(Some(info));
            }
        }
        LevelInfo::new(None)
    }

    /// The component named `component_name`, ignoring case; invalid if none.
    pub fn component_info_by_name(&self, component_name: &[u8]) -> ComponentInfo {
        for (idx, info) in self.infos.iter().enumerate() {
            if compare_ignore_case(component_name, info.info.name().string()) == 0 {
                return ComponentInfo::new(idx as u8, Some(&info.info));
            }
        }
        ComponentInfo::default()
    }
}

impl<const N: usize> ComponentMappingTrait for ComponentMapping<N> {
    fn is_enabled(&self, component_index: u8, level: Level) -> bool {
        self.resolve_global();
        usize::from(component_index) < N
            && level as u8 >= self.levels[usize::from(component_index)].load(Ordering::Relaxed)
            && level as u8 >= self.global_level.load(Ordering::Relaxed)
    }

    fn level(&self, component_index: u8) -> Level {
        if usize::from(component_index) < N {
            Level::from_u8(self.levels[usize::from(component_index)].load(Ordering::Relaxed))
        } else {
            Level::None
        }
    }

    fn level_info(&self, level: Level) -> LevelInfo {
        let index = level as usize;
        LevelInfo::new(if index < self.level_infos.len() && index < LEVEL_COUNT {
            Some(&self.level_infos[index])
        } else {
            None
        })
    }

    fn component_info(&self, component_index: u8) -> ComponentInfo {
        ComponentInfo::new(
            component_index,
            self.infos.get(usize::from(component_index)).map(|info| &info.info),
        )
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use openbsw_util::format::Color;

    pub(crate) static MAP1: LoggerComponent = LoggerComponent::new();
    pub(crate) static MAP2: LoggerComponent = LoggerComponent::new();
    pub(crate) static MAP3: LoggerComponent = LoggerComponent::new();
    pub(crate) static EMPTY_COMP: LoggerComponent = LoggerComponent::new();

    const PLAIN: StringAttributes = StringAttributes::color(Color::DefaultColor);

    pub(crate) static INFOS: [MappingInfo; 3] = [
        MappingInfo::new(Level::Info, &MAP1, b"MAP1", PLAIN),
        MappingInfo::new(Level::Debug, &MAP2, b"MAP2", PLAIN),
        MappingInfo::new(Level::Error, &MAP3, b"MAP3", PLAIN),
    ];

    fn levels() -> &'static [PlainLevelInfo] {
        LevelInfo::default_table()
    }

    // Ported from logger/test/src/logger/ComponentMappingTest.cpp.
    #[test]
    fn macro_defined_mapping() {
        let cut: ComponentMapping<3> = ComponentMapping::new(&INFOS, levels(), Some(&MAP1));
        assert_eq!(cut.mapping_size(), 3);
        assert_eq!(cut.level(EMPTY_COMP.index()), Level::None);
        assert!(!cut.is_enabled(EMPTY_COMP.index(), Level::Critical));
        assert!(cut.component_info(0).is_valid());
        assert!(cut.component_info(1).is_valid());
        assert!(cut.component_info(2).is_valid());
        assert!(!cut.component_info(3).is_valid());
        cut.apply_mapping();
        assert_eq!(cut.level(MAP1.index()), Level::Info);
        assert_eq!(cut.level(MAP2.index()), Level::Debug);
        assert_eq!(cut.level(MAP3.index()), Level::Error);
        assert_eq!(cut.level(3), Level::None);
        assert!(cut.is_enabled(MAP1.index(), Level::Info));
        // The global component's level gates every other one.
        assert!(!cut.is_enabled(MAP2.index(), Level::Debug));
        assert!(cut.is_enabled(MAP2.index(), Level::Info));
        assert!(!cut.is_enabled(MAP3.index(), Level::Info));
        assert!(cut.is_enabled(MAP3.index(), Level::Error));
        cut.clear_mapping();
        assert_eq!(cut.level(MAP1.index()), Level::None);
        assert_eq!(cut.level(MAP2.index()), Level::None);
        assert_eq!(cut.level(MAP3.index()), Level::None);
    }

    // The global component's level gates messages before the mapping is applied, as the
    // C++ constructor sets it.
    #[test]
    fn global_level_applies_before_apply_mapping() {
        let cut: ComponentMapping<3> = ComponentMapping::new(&INFOS, levels(), Some(&MAP1));
        // MAP2 has no index yet; ask by table position. MAP1, the global, is at Info.
        assert!(!cut.is_enabled(1, Level::Debug));
        assert!(cut.is_enabled(1, Level::Info));
        // Setting the global's level before the mapping is applied moves the gate.
        cut.set_level(0, Level::Error);
        assert!(!cut.is_enabled(1, Level::Info));
        assert!(cut.is_enabled(1, Level::Error));
        cut.apply_mapping();
        assert!(!cut.is_enabled(MAP2.index(), Level::Info));
        cut.clear_mapping();
    }

    #[test]
    fn non_global_mapping() {
        let cut: ComponentMapping<3> = ComponentMapping::new(&INFOS, levels(), Some(&EMPTY_COMP));
        cut.apply_mapping();
        cut.set_level(MAP1.index(), Level::Error);
        assert!(!cut.is_enabled(MAP1.index(), Level::Info));
        assert!(cut.is_enabled(MAP1.index(), Level::Error));
        cut.clear_mapping();
    }

    #[test]
    fn level_info_handling() {
        let cut: ComponentMapping<3> = ComponentMapping::new(&INFOS, levels(), Some(&EMPTY_COMP));
        assert_eq!(cut.level_info(Level::Debug).name().string(), b"DEBUG");
        assert!(
            !cut.level_info(Level::from_u8(LEVEL_COUNT as u8)).is_valid()
                || Level::from_u8(LEVEL_COUNT as u8) == Level::None
        );
        assert_eq!(cut.level_info_by_name(b"debug").name().string(), b"DEBUG");
        assert_eq!(cut.level_info_by_name(b"DEBUG").name().string(), b"DEBUG");
        assert!(!cut.level_info_by_name(b"TEST").is_valid());
    }

    #[test]
    fn component_info_handling() {
        let cut: ComponentMapping<3> = ComponentMapping::new(&INFOS, levels(), Some(&EMPTY_COMP));
        assert_eq!(cut.component_info(2).name().string(), b"MAP3");
        assert!(!cut.component_info(3).is_valid());
        assert_eq!(cut.component_info_by_name(b"map3").name().string(), b"MAP3");
        assert_eq!(cut.component_info_by_name(b"MAP3").name().string(), b"MAP3");
        assert!(!cut.component_info_by_name(b"TEST").is_valid());
    }

    #[test]
    fn bad_values() {
        let cut: ComponentMapping<3> = ComponentMapping::new(&INFOS, levels(), Some(&EMPTY_COMP));
        cut.apply_mapping();
        // The C++ test passes LEVEL_COUNT, which the enum cannot express; an unknown
        // component is ignored the same way.
        cut.set_level(3, Level::Error);
        assert_eq!(cut.level(MAP1.index()), Level::Info);
        assert_eq!(cut.level(3), Level::None);
        cut.clear_mapping();
    }
}
