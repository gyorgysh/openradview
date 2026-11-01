// SPDX-License-Identifier: GPL-3.0-or-later
//! One ready source image at a time. Decoding never creates a catch-up queue.
mod ui;
use orv_core::acquisition::CineTiming;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Step {
    Idle,
    Wait(Duration),
    Advance(u32),
}

struct Shown {
    deadline: Instant,
    ready: Instant,
    interval: Duration,
}
struct Requested {
    at: Instant,
    previous_ready: Instant,
    interval: Duration,
}
#[derive(Debug, PartialEq, Eq)]
struct Pace {
    elapsed: Duration,
    interval: Duration,
    image_wait: Duration,
}

pub(super) struct Playback {
    pub running: bool,
    pub fps: u8,
    pub repeat: bool,
    source: Option<Arc<CineTiming>>,
    source_mode: bool,
    expected: Option<u32>,
    shown: Option<Shown>,
    requested: Option<Requested>,
    pace: Option<Pace>,
    notice: Option<&'static str>,
}

impl Default for Playback {
    fn default() -> Self {
        Self {
            running: false,
            fps: 8,
            repeat: false,
            source: None,
            source_mode: false,
            expected: None,
            shown: None,
            requested: None,
            pace: None,
            notice: None,
        }
    }
}

impl Playback {
    /// Metadata is a capability for this catalogued source, not equal numeric timing.
    /// Return whether an enabled mode was reset so the owner can revoke consent.
    pub fn bind_source(&mut self, source: Option<Arc<CineTiming>>) -> bool {
        let same = match (&self.source, &source) {
            (Some(old), Some(new)) => Arc::ptr_eq(old, new),
            (None, None) => true,
            _ => false,
        };
        if same {
            return false;
        }
        let changed = self.source_mode;
        self.pause();
        self.source_mode = false;
        self.source = source;
        self.pace = None;
        self.notice = None;
        changed
    }
    pub fn set_source_mode(&mut self, enabled: bool) -> bool {
        let enabled = enabled && self.source_playable();
        if enabled == self.source_mode {
            return false;
        }
        self.pause();
        self.source_mode = enabled;
        self.pace = None;
        self.notice = None;
        true
    }
    fn source_playable(&self) -> bool {
        self.source.as_ref().is_some_and(|timing| {
            let [first, last] = timing.trim();
            first < last
        })
    }
    pub fn pause(&mut self) {
        self.running = false;
        self.expected = None;
        self.shown = None;
        self.requested = None;
        self.notice = None;
    }

    /// Starting at/outside the end returns to the first playable image. Resuming
    /// within the range gives that image a full interval; acquisition delay is not
    /// a playback dwell and no interval after the final trimmed image is invented.
    pub fn start_at(&mut self, target: u32, count: usize) -> Option<u32> {
        self.pause();
        self.pace = None;
        self.notice = None;
        if count < 2 || target as usize >= count {
            return None;
        }
        let target = if self.source_mode {
            let timing = self.source.as_ref()?;
            if timing.frame_count() as usize != count || !self.source_playable() {
                return None;
            }
            let [first, last] = timing.trim();
            if target < first || target >= last {
                first
            } else {
                target
            }
        } else if target as usize == count - 1 {
            0
        } else {
            target
        };
        self.running = true;
        self.expected = Some(target);
        Some(target)
    }

    fn fail(&mut self, notice: &'static str) -> Step {
        self.pause();
        self.notice = Some(notice);
        Step::Idle
    }
    pub fn tick(
        &mut self,
        now: Instant,
        target: u32,
        displayed: Option<usize>,
        count: usize,
    ) -> Step {
        if !self.running {
            return Step::Idle;
        }
        if count < 2 || target as usize >= count || self.expected != Some(target) {
            return self.fail("Playback paused because the requested image context changed.");
        }
        let next = if self.source_mode {
            let Some(timing) = self.source.as_ref() else {
                return self.fail("Source timing is no longer available. Playback paused.");
            };
            let [first, last] = timing.trim();
            if timing.frame_count() as usize != count || !(first..=last).contains(&target) {
                return self
                    .fail("Source timing does not match this image range. Playback paused.");
            }
            target.checked_add(1).filter(|next| *next <= last)
        } else if target as usize + 1 < count {
            Some(target + 1)
        } else if self.repeat {
            Some(0)
        } else {
            None
        };
        if displayed != Some(target as usize) {
            // The worker wakes the UI. No timer or new request while decoding.
            self.shown = None;
            return Step::Idle;
        }
        if let Some(requested) = self.requested.take() {
            self.pace = Some(Pace {
                elapsed: now.saturating_duration_since(requested.previous_ready),
                interval: requested.interval,
                image_wait: now.saturating_duration_since(requested.at),
            });
        }
        if self.source_mode && next.is_none() {
            self.pause();
            self.notice = Some("Reached the source trim end.");
            return Step::Idle;
        }
        if self.shown.is_none() {
            let interval = if self.source_mode {
                let Some(interval) = next
                    .and_then(|frame| self.source.as_ref()?.interval_before(frame))
                    .filter(|d| !d.is_zero())
                else {
                    return self.fail("Source interval is unavailable. Playback paused.");
                };
                interval
            } else {
                Duration::from_secs_f64(1.0 / f64::from(self.fps.clamp(1, 30)))
            };
            let Some(deadline) = now.checked_add(interval) else {
                return self
                    .fail("Source interval exceeds the monotonic clock range. Playback paused.");
            };
            self.shown = Some(Shown {
                deadline,
                ready: now,
                interval,
            });
        }
        let Some(shown) = &self.shown else {
            return Step::Idle;
        };
        if now < shown.deadline {
            return Step::Wait(shown.deadline.duration_since(now));
        }
        if let Some(next) = next {
            self.requested = Some(Requested {
                at: now,
                previous_ready: shown.ready,
                interval: shown.interval,
            });
            self.shown = None;
            self.expected = Some(next);
            Step::Advance(next)
        } else {
            self.pause();
            Step::Idle
        }
    }
}

#[cfg(test)]
mod tests;
