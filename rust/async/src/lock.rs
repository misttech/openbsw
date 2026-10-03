// Copyright 2026 Mist Tecnologia LTDA. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Critical sections, ported from the Zephyr adaptation's `async/Lock.h` and
//! `async/ModifiableLock.h`, over a platform [`RawLock`].

use core::marker::PhantomData;

use openbsw_timer::Lock;

/// The platform's interrupt lock: `irq_lock`/`irq_unlock` on Zephyr.
pub trait RawLock {
    /// Lock; returns the key to unlock with.
    fn acquire() -> u32;
    /// Unlock with the key from `acquire`.
    fn release(key: u32);
}

/// A raw lock that does nothing, for hosts and tests.
pub struct NoRawLock;

impl RawLock for NoRawLock {
    fn acquire() -> u32 {
        0
    }

    fn release(_key: u32) {}
}

/// A critical section held for a scope: the port of `async::Lock`.
pub struct ScopedLock<R: RawLock> {
    key: u32,
    _raw: PhantomData<R>,
}

impl<R: RawLock> Lock for ScopedLock<R> {
    fn lock() -> Self {
        Self { key: R::acquire(), _raw: PhantomData }
    }
}

impl<R: RawLock> Drop for ScopedLock<R> {
    fn drop(&mut self) {
        R::release(self.key);
    }
}

/// A critical section that can be released and retaken within its scope: the port of
/// `async::ModifiableLock`. It is locked on creation and released on drop if still held.
pub struct ModifiableLock<R: RawLock> {
    key: u32,
    locked: bool,
    _raw: PhantomData<R>,
}

impl<R: RawLock> ModifiableLock<R> {
    /// Take the lock.
    pub fn new() -> Self {
        Self { key: R::acquire(), locked: true, _raw: PhantomData }
    }

    /// Release the lock if held.
    pub fn unlock(&mut self) {
        if self.locked {
            R::release(self.key);
            self.locked = false;
        }
    }

    /// Retake the lock if released.
    pub fn lock(&mut self) {
        if !self.locked {
            self.key = R::acquire();
            self.locked = true;
        }
    }
}

impl<R: RawLock> Default for ModifiableLock<R> {
    fn default() -> Self {
        Self::new()
    }
}

impl<R: RawLock> Drop for ModifiableLock<R> {
    fn drop(&mut self) {
        if self.locked {
            R::release(self.key);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use core::cell::Cell;

    std::thread_local! {
        pub(crate) static LOCKS: Cell<u32> = const { Cell::new(0) };
        pub(crate) static UNLOCKS: Cell<u32> = const { Cell::new(0) };
    }

    /// Counts lock and unlock calls, the port of `LockMock`.
    pub(crate) struct CountingRawLock;

    impl RawLock for CountingRawLock {
        fn acquire() -> u32 {
            LOCKS.with(|locks| locks.set(locks.get() + 1));
            7
        }

        fn release(key: u32) {
            assert_eq!(key, 7);
            UNLOCKS.with(|unlocks| unlocks.set(unlocks.get() + 1));
        }
    }

    pub(crate) fn counts() -> (u32, u32) {
        (LOCKS.with(|l| l.get()), UNLOCKS.with(|u| u.get()))
    }

    // Ported from async/test/src/async/TypesTest.cpp (testLockType, testModifiableLockType).
    #[test]
    fn scoped_lock_locks_and_unlocks() {
        let before = counts();
        {
            let _cut = ScopedLock::<CountingRawLock>::lock();
            assert_eq!(counts(), (before.0 + 1, before.1));
        }
        assert_eq!(counts(), (before.0 + 1, before.1 + 1));
    }

    #[test]
    fn modifiable_lock() {
        let before = counts();
        {
            let mut cut = ModifiableLock::<CountingRawLock>::new();
            assert_eq!(counts(), (before.0 + 1, before.1));
            // No lock while already locked.
            cut.lock();
            assert_eq!(counts(), (before.0 + 1, before.1));
            // Unlock on the first call only.
            cut.unlock();
            cut.unlock();
            assert_eq!(counts(), (before.0 + 1, before.1 + 1));
            // Lock again, and the drop unlocks.
            cut.lock();
            assert_eq!(counts(), (before.0 + 2, before.1 + 1));
        }
        assert_eq!(counts(), (before.0 + 2, before.1 + 2));
        {
            let mut cut = ModifiableLock::<CountingRawLock>::new();
            cut.unlock();
        }
        // An unlocked lock does not unlock again on drop.
        assert_eq!(counts(), (before.0 + 3, before.1 + 3));
    }
}
