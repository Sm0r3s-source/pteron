/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Pteron: a lock that keeps working after another thread panicked while holding it.
//! (A plain `lock().unwrap()` would make every later user panic too, which could stop all video.)

use std::sync::{Mutex, MutexGuard};

pub(crate) trait LockExt<T> {
    fn lock_ok(&self) -> MutexGuard<'_, T>;
}

impl<T> LockExt<T> for Mutex<T> {
    fn lock_ok(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_poisoned_lock_still_opens() {
        let lock = std::sync::Arc::new(Mutex::new(5));
        let other = lock.clone();
        let _ = std::thread::spawn(move || {
            let _guard = other.lock().unwrap();
            panic!("on purpose");
        })
        .join();
        assert!(lock.lock().is_err());
        assert_eq!(*lock.lock_ok(), 5);
    }
}
