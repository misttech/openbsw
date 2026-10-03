// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! How UDS asks the application to reset or power down: the port of
//! `lifecycle/IUdsLifecycleConnector.h`.

/// The kinds of shutdown a diagnostic request can ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShutdownType {
    /// None.
    NoShutdown,
    /// Power down.
    PowerDown,
    /// Hard reset.
    HardReset,
    /// Soft reset.
    SoftReset,
    /// A reset that destroys the software state.
    SoftwareDestructiveReset,
    /// Jump to the updater.
    GotoUpdater,
    /// Update the bootloader.
    BootloaderUpdate,
}

/// The application's side of a mode change.
pub trait UdsLifecycleConnector: Sync {
    /// Whether a mode change may happen now.
    fn is_mode_change_possible(&self) -> bool;
    /// Ask for a power down; returns whether it was accepted and sets the time it takes.
    fn request_powerdown(&self, rapid: bool, time: &mut u8) -> bool;
    /// Ask for a shutdown of `shutdown_type` after `timeout` milliseconds.
    fn request_shutdown(&self, shutdown_type: ShutdownType, timeout: u32) -> bool;
}
