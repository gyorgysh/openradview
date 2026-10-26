// SPDX-License-Identifier: GPL-3.0-or-later
//! User-paced source browsing, independent of acquisition timing. A ready image
//! receives a full interval before the next request; late decoding never causes catch-up skips.
use std::time::{Duration, Instant};

pub(super) enum Step {
    Idle,
    Wait(Duration),
    Advance(u32),
}

pub(super) struct Playback {
    pub running: bool,
    pub fps: u8,
    pub repeat: bool,
    shown: Option<(u32, Instant)>,
}

impl Default for Playback {
    fn default() -> Self {
        Self {
            running: false,
            fps: 8,
            repeat: false,
            shown: None,
        }
    }
}

impl Playback {
    pub fn pause(&mut self) {
        self.running = false;
        self.shown = None;
    }

    pub fn start(&mut self) {
        self.running = true;
        self.shown = None;
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
        if count < 2 || target as usize >= count {
            self.pause();
            return Step::Idle;
        }
        if displayed != Some(target as usize) {
            // The decode worker wakes the UI. No timer or newer decode request
            // is needed while waiting for the current target.
            self.shown = None;
            return Step::Idle;
        }
        let (_, since) = *self.shown.get_or_insert((target, now));
        if self.shown.is_some_and(|(index, _)| index != target) {
            self.shown = Some((target, now));
            return Step::Wait(self.interval());
        }
        let elapsed = now.saturating_duration_since(since);
        let interval = self.interval();
        if elapsed < interval {
            return Step::Wait(interval - elapsed);
        }
        self.shown = None;
        if (target as usize) + 1 < count {
            Step::Advance(target + 1)
        } else if self.repeat {
            Step::Advance(0)
        } else {
            self.pause();
            Step::Idle
        }
    }

    fn interval(&self) -> Duration {
        Duration::from_secs_f64(1.0 / f64::from(self.fps.clamp(1, 30)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn slow_decode_receives_full_interval_and_never_skips() {
        let now = Instant::now();
        let mut player = Playback::default();
        player.start();
        assert!(matches!(player.tick(now, 0, Some(0), 100), Step::Wait(_)));
        assert!(matches!(
            player.tick(now + Duration::from_secs(5), 0, Some(0), 100),
            Step::Advance(1)
        ));
        assert!(matches!(
            player.tick(now + Duration::from_secs(20), 1, Some(0), 100),
            Step::Idle
        ));
        assert!(player.running);
        let ready = now + Duration::from_secs(30);
        assert!(
            matches!(player.tick(ready, 1, Some(1), 100), Step::Wait(d) if d == Duration::from_millis(125))
        );
        assert!(matches!(
            player.tick(ready + Duration::from_millis(124), 1, Some(1), 100),
            Step::Wait(_)
        ));
        assert!(matches!(
            player.tick(ready + Duration::from_millis(125), 1, Some(1), 100),
            Step::Advance(2)
        ));
    }
    #[test]
    fn end_stops_unless_loop_is_explicit_and_pause_is_idle() {
        let now = Instant::now();
        let mut player = Playback::default();
        player.start();
        player.tick(now, 2, Some(2), 3);
        assert!(matches!(
            player.tick(now + Duration::from_secs(1), 2, Some(2), 3),
            Step::Idle
        ));
        assert!(!player.running);
        player.repeat = true;
        player.start();
        player.tick(now, 2, Some(2), 3);
        assert!(matches!(
            player.tick(now + Duration::from_secs(1), 2, Some(2), 3),
            Step::Advance(0)
        ));
        player.pause();
        assert!(matches!(player.tick(now, 0, Some(0), 3), Step::Idle));
        player.start();
        assert!(matches!(player.tick(now, 0, Some(0), 1), Step::Idle));
        assert!(!player.running);
    }
}
