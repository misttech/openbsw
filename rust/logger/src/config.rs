// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Connecting the mapping to the logger facade, ported from `logger/ComponentConfig.h`.

use openbsw_util::logger::{
    ComponentInfo, ComponentMapping as ComponentMappingTrait, Level, LevelInfo, Logger,
    LoggerOutput,
};

use crate::mapping::ComponentMapping;

/// Starts and stops logging through a [`ComponentMapping`]: the port of
/// `ComponentConfig<IndexUpperBound>`. Levels are not persisted (`readLevels` and
/// `writeLevels` are empty in C++).
pub struct ComponentConfig<const N: usize> {
    mapping: &'static ComponentMapping<N>,
}

impl<const N: usize> ComponentConfig<N> {
    /// A config over `mapping`.
    pub const fn new(mapping: &'static ComponentMapping<N>) -> Self {
        Self { mapping }
    }

    /// Assign the component indices and connect the facade to the mapping and `output`.
    pub fn start(&self, output: &'static dyn LoggerOutput) {
        self.mapping.apply_mapping();
        self.read_levels();
        Logger::init(self.mapping, output);
    }

    /// Disconnect the facade and unassign the components.
    pub fn shutdown(&self) {
        Logger::shutdown();
        self.mapping.clear_mapping();
        self.write_levels();
    }

    /// The number of components.
    pub fn mapping_size(&self) -> u8 {
        self.mapping.mapping_size()
    }

    /// A component's level.
    pub fn level(&self, component_index: u8) -> Level {
        self.mapping.level(component_index)
    }

    /// Set a component's level.
    pub fn set_level(&self, component_index: u8, level: Level) {
        self.mapping.set_level(component_index, level);
    }

    /// Information about a level.
    pub fn level_info(&self, level: Level) -> LevelInfo {
        self.mapping.level_info(level)
    }

    /// The level named `level_name`.
    pub fn level_info_by_name(&self, level_name: &[u8]) -> LevelInfo {
        self.mapping.level_info_by_name(level_name)
    }

    /// Information about a component.
    pub fn component_info(&self, component_index: u8) -> ComponentInfo {
        self.mapping.component_info(component_index)
    }

    /// The component named `component_name`.
    pub fn component_info_by_name(&self, component_name: &[u8]) -> ComponentInfo {
        self.mapping.component_info_by_name(component_name)
    }

    /// Load the levels from persistence; nothing is persisted.
    pub fn read_levels(&self) {}

    /// Store the levels to persistence; nothing is persisted.
    pub fn write_levels(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::MappingInfo;
    use openbsw_util::format::{Arg, Color, StringAttributes};
    use openbsw_util::logger::{COMPONENT_NONE, LoggerComponent};

    struct NullOutput;

    impl LoggerOutput for NullOutput {
        fn log_output(
            &self,
            _component_info: &ComponentInfo,
            _level_info: &LevelInfo,
            _format: &[u8],
            _args: &[Arg<'_>],
        ) {
        }
    }

    static CONF1: LoggerComponent = LoggerComponent::new();
    static CONF2: LoggerComponent = LoggerComponent::new();
    static CONF3: LoggerComponent = LoggerComponent::new();
    const PLAIN: StringAttributes = StringAttributes::color(Color::DefaultColor);
    static INFOS: [MappingInfo; 3] = [
        MappingInfo::new(Level::None, &CONF1, b"_CONF1", PLAIN),
        MappingInfo::new(Level::None, &CONF2, b"_CONF2", PLAIN),
        MappingInfo::new(Level::None, &CONF3, b"_CONF3", PLAIN),
    ];
    static MAPPING: ComponentMapping<3> =
        ComponentMapping::new(&INFOS, LevelInfo::default_table(), Some(&CONF1));
    static OUTPUT: NullOutput = NullOutput;

    // Ported from logger/test/src/logger/ComponentConfigTest.cpp.
    #[test]
    fn basic_functionality() {
        let _serial = crate::LOGGER_SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let cut = ComponentConfig::new(&MAPPING);
        cut.start(&OUTPUT);
        assert_eq!(cut.mapping_size(), 3);
        assert_eq!(CONF1.index(), 0);
        assert_eq!(CONF2.index(), 1);
        assert_eq!(CONF3.index(), 2);
        assert!(cut.component_info(CONF1.index()).is_valid());
        assert_eq!(cut.component_info_by_name(b"_CONF1").name().string(), b"_CONF1");
        assert_eq!(cut.level_info(Level::Debug).name().string(), b"DEBUG");
        assert_eq!(cut.level_info_by_name(b"DEBUG").name().string(), b"DEBUG");
        assert_eq!(cut.level(CONF1.index()), Level::None);
        cut.set_level(CONF1.index(), Level::Info);
        assert_eq!(cut.level(CONF1.index()), Level::Info);
        cut.shutdown();
        assert_eq!(CONF1.index(), COMPONENT_NONE);
        assert_eq!(CONF2.index(), COMPONENT_NONE);
        assert_eq!(CONF3.index(), COMPONENT_NONE);
    }
}
