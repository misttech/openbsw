// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! A runnable around a function, ported from `async/util/Call.h` (`async::Function`).

use crate::queue::QueueNode;
use crate::types::Runnable;

/// A runnable that calls a function: the port of `async::Function`.
pub struct FunctionRunnable {
    call: &'static (dyn Fn() + Sync),
    node: QueueNode<dyn Runnable>,
}

impl FunctionRunnable {
    /// A runnable calling `call`.
    pub const fn new(call: &'static (dyn Fn() + Sync)) -> Self {
        Self { call, node: QueueNode::new() }
    }
}

impl Runnable for FunctionRunnable {
    fn execute(&self) {
        (self.call)();
    }

    fn node(&self) -> &QueueNode<dyn Runnable> {
        &self.node
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicU32, Ordering};

    static CALLS: AtomicU32 = AtomicU32::new(0);

    fn called() {
        CALLS.fetch_add(1, Ordering::Relaxed);
    }

    // Ported from async/test/src/async/util/CallTest.cpp.
    #[test]
    fn executes_the_function() {
        static CUT: FunctionRunnable = FunctionRunnable::new(&called);
        CUT.execute();
        CUT.execute();
        assert_eq!(CALLS.load(Ordering::Relaxed), 2);
        assert!(!CUT.node().is_enqueued());
    }
}
