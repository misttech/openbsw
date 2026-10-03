// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Run time statistics, ported from `RuntimeStatistics.h` and
//! `FunctionRuntimeStatistics.h`.
//!
//! The fields are cells: an entry is a `static` updated from the context-switch hooks
//! under the platform lock, which is the C++ contract.

use core::cell::Cell;

/// What an entry's statistics collect: the port of the `addRun`/`reset` the stack entries
/// call on their `Statistics` base.
pub trait Statistics: Sync + 'static {
    /// A run that started at `start_timestamp` took `runtime`, with `suspended_time` of
    /// it spent in what preempted it.
    fn add_run(&self, start_timestamp: u32, runtime: u32, suspended_time: u32);
    /// Forget every run.
    fn reset(&self);
    /// Take the values of `other`.
    fn copy_from(&self, other: &Self);
}

/// Total, count, minimum and maximum of run times.
#[derive(Debug, Default)]
pub struct RuntimeStatistics {
    total_runtime: Cell<u32>,
    total_run_count: Cell<u32>,
    min_runtime: Cell<u32>,
    max_runtime: Cell<u32>,
}

// SAFETY: see the module documentation: updated under the platform lock.
unsafe impl Sync for RuntimeStatistics {}

impl RuntimeStatistics {
    /// No runs yet.
    pub const fn new() -> Self {
        Self {
            total_runtime: Cell::new(0),
            total_run_count: Cell::new(0),
            min_runtime: Cell::new(0),
            max_runtime: Cell::new(0),
        }
    }

    /// Add a run of `runtime`.
    pub fn add_runtime(&self, runtime: u32) {
        self.total_runtime.set(self.total_runtime.get().wrapping_add(runtime));
        if self.total_run_count.get() == 0 {
            self.min_runtime.set(runtime);
            self.max_runtime.set(runtime);
        } else {
            if self.min_runtime.get() > runtime {
                self.min_runtime.set(runtime);
            }
            if self.max_runtime.get() < runtime {
                self.max_runtime.set(runtime);
            }
        }
        self.total_run_count.set(self.total_run_count.get().wrapping_add(1));
    }

    /// The sum of the run times.
    pub fn total_runtime(&self) -> u32 {
        self.total_runtime.get()
    }

    /// The number of runs.
    pub fn total_run_count(&self) -> u32 {
        self.total_run_count.get()
    }

    /// The shortest run.
    pub fn min_runtime(&self) -> u32 {
        self.min_runtime.get()
    }

    /// The longest run.
    pub fn max_runtime(&self) -> u32 {
        self.max_runtime.get()
    }

    /// The mean run time, 0 without runs.
    pub fn average_runtime(&self) -> u32 {
        match self.total_run_count.get() {
            0 => 0,
            count => self.total_runtime.get() / count,
        }
    }
}

impl Statistics for RuntimeStatistics {
    fn add_run(&self, _start_timestamp: u32, runtime: u32, _suspended_time: u32) {
        self.add_runtime(runtime);
    }

    fn reset(&self) {
        self.total_runtime.set(0);
        self.total_run_count.set(0);
        self.min_runtime.set(0);
        self.max_runtime.set(0);
    }

    fn copy_from(&self, other: &Self) {
        self.total_runtime.set(other.total_runtime.get());
        self.total_run_count.set(other.total_run_count.get());
        self.min_runtime.set(other.min_runtime.get());
        self.max_runtime.set(other.max_runtime.get());
    }
}

/// [`RuntimeStatistics`] plus the jitter between consecutive starts.
#[derive(Debug, Default)]
pub struct FunctionRuntimeStatistics {
    base: RuntimeStatistics,
    min_jitter: Cell<u32>,
    max_jitter: Cell<u32>,
    prev_timestamp: Cell<u32>,
}

// SAFETY: as `RuntimeStatistics`.
unsafe impl Sync for FunctionRuntimeStatistics {}

impl FunctionRuntimeStatistics {
    /// No runs yet.
    pub const fn new() -> Self {
        Self {
            base: RuntimeStatistics::new(),
            min_jitter: Cell::new(0),
            max_jitter: Cell::new(0),
            prev_timestamp: Cell::new(0),
        }
    }

    /// Add a run that started at `start_timestamp` and took `runtime`.
    pub fn add_timed_run(&self, start_timestamp: u32, runtime: u32) {
        let total_run_count = self.base.total_run_count();
        if total_run_count > 0 {
            let jitter = start_timestamp.wrapping_sub(self.prev_timestamp.get());
            if total_run_count == 1 {
                self.min_jitter.set(jitter);
                self.max_jitter.set(jitter);
            } else {
                if jitter < self.min_jitter.get() {
                    self.min_jitter.set(jitter);
                }
                if jitter > self.max_jitter.get() {
                    self.max_jitter.set(jitter);
                }
            }
        }
        self.prev_timestamp.set(start_timestamp);
        self.base.add_runtime(runtime);
    }

    /// The run time statistics.
    pub fn runtime(&self) -> &RuntimeStatistics {
        &self.base
    }

    /// The smallest gap between two starts.
    pub fn min_jitter(&self) -> u32 {
        self.min_jitter.get()
    }

    /// The largest gap between two starts.
    pub fn max_jitter(&self) -> u32 {
        self.max_jitter.get()
    }
}

impl Statistics for FunctionRuntimeStatistics {
    fn add_run(&self, start_timestamp: u32, runtime: u32, _suspended_time: u32) {
        self.add_timed_run(start_timestamp, runtime);
    }

    fn reset(&self) {
        self.base.reset();
        self.min_jitter.set(0);
        self.max_jitter.set(0);
        self.prev_timestamp.set(0);
    }

    fn copy_from(&self, other: &Self) {
        self.base.copy_from(&other.base);
        self.min_jitter.set(other.min_jitter.get());
        self.max_jitter.set(other.max_jitter.get());
        self.prev_timestamp.set(other.prev_timestamp.get());
    }
}

// Ported from runtime/test/src/RuntimeStatisticsTest.cpp and
// FunctionRuntimeStatisticsTest.cpp.
#[cfg(test)]
mod tests {
    use super::*;

    fn values(cut: &RuntimeStatistics) -> [u32; 5] {
        [
            cut.total_runtime(),
            cut.total_run_count(),
            cut.min_runtime(),
            cut.max_runtime(),
            cut.average_runtime(),
        ]
    }

    #[test]
    fn runtime_statistics_constructor() {
        assert_eq!(values(&RuntimeStatistics::new()), [0; 5]);
    }

    #[test]
    fn runtime_statistics_add_run() {
        let cut = RuntimeStatistics::new();
        cut.add_runtime(15);
        assert_eq!(values(&cut), [15, 1, 15, 15, 15]);
        cut.add_runtime(30);
        assert_eq!(values(&cut), [45, 2, 15, 30, 22]);
        cut.add_runtime(10);
        assert_eq!(values(&cut), [55, 3, 10, 30, 18]);
        cut.add_run(500, 20, 7500);
        assert_eq!(values(&cut), [75, 4, 10, 30, 18]);
    }

    #[test]
    fn runtime_statistics_reset() {
        let cut = RuntimeStatistics::new();
        cut.add_runtime(15);
        assert_eq!(values(&cut), [15, 1, 15, 15, 15]);
        cut.reset();
        assert_eq!(values(&cut), [0; 5]);
        let copy = RuntimeStatistics::new();
        cut.add_runtime(7);
        copy.copy_from(&cut);
        assert_eq!(values(&copy), [7, 1, 7, 7, 7]);
    }

    fn function_values(cut: &FunctionRuntimeStatistics) -> [u32; 7] {
        let [total, count, min, max, average] = values(cut.runtime());
        [total, count, min, max, average, cut.min_jitter(), cut.max_jitter()]
    }

    #[test]
    fn function_statistics_constructor() {
        assert_eq!(function_values(&FunctionRuntimeStatistics::new()), [0; 7]);
    }

    #[test]
    fn function_statistics_add_run() {
        let cut = FunctionRuntimeStatistics::new();
        cut.add_timed_run(25, 15);
        assert_eq!(function_values(&cut), [15, 1, 15, 15, 15, 0, 0]);
        cut.add_timed_run(45, 30);
        assert_eq!(function_values(&cut), [45, 2, 15, 30, 22, 20, 20]);
        cut.add_timed_run(85, 10);
        assert_eq!(function_values(&cut), [55, 3, 10, 30, 18, 20, 40]);
        cut.add_run(100, 20, 7500);
        assert_eq!(function_values(&cut), [75, 4, 10, 30, 18, 15, 40]);
    }

    #[test]
    fn function_statistics_reset() {
        let cut = FunctionRuntimeStatistics::new();
        cut.add_timed_run(20, 14);
        cut.add_timed_run(45, 16);
        assert_eq!(function_values(&cut), [30, 2, 14, 16, 15, 25, 25]);
        cut.reset();
        assert_eq!(function_values(&cut), [0; 7]);
    }
}
