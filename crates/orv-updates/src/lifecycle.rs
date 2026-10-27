// SPDX-License-Identifier: GPL-3.0-or-later
//! An ordinary app quit cannot interrupt the directory/executable rename window.
use std::sync::atomic::{AtomicU8, Ordering};
const RUNNING: u8 = 0;
const CLOSING: u8 = 1;
const COMMITTING: u8 = 2;
#[derive(Default)]
pub(crate) struct Lifecycle(AtomicU8);
pub(crate) struct Commit<'a>(&'a Lifecycle);
impl Lifecycle {
    pub fn closing(&self) -> bool {
        self.0.load(Ordering::SeqCst) == CLOSING
    }
    pub fn request_close(&self) -> bool {
        self.0
            .compare_exchange(RUNNING, CLOSING, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
            || self.0.load(Ordering::SeqCst) == CLOSING
    }
    pub fn begin_commit(&self) -> Result<Commit<'_>, &'static str> {
        self.0
            .compare_exchange(RUNNING, COMMITTING, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| "Installation was canceled because the app is closing.")?;
        Ok(Commit(self))
    }
}
impl Drop for Commit<'_> {
    fn drop(&mut self) {
        self.0.0.store(RUNNING, Ordering::SeqCst);
    }
}
