// SPDX-License-Identifier: GPL-3.0-or-later
//! Input ownership only; every materialization happens inside a worker.
use orv_core::{
    study::{Frame, Series},
    volume::Volume,
};
use std::sync::Arc;

pub enum FrameSource {
    Resident(Vec<Arc<Frame>>),
    Indexed(Arc<Series>),
}
impl From<Vec<Arc<Frame>>> for FrameSource {
    fn from(frames: Vec<Arc<Frame>>) -> Self {
        Self::Resident(frames)
    }
}
impl From<Arc<Series>> for FrameSource {
    fn from(series: Arc<Series>) -> Self {
        Self::Indexed(series)
    }
}
impl FrameSource {
    pub fn original_window(&self) -> Option<orv_core::WindowLevel> {
        match self {
            Self::Resident(frames) => frames.get(frames.len() / 2).map(|f| f.default_window),
            Self::Indexed(series) => series
                .source
                .as_ref()
                .and_then(|s| s.original_window(s.len() / 2))
                .or_else(|| {
                    series
                        .frames
                        .get(series.frames.len() / 2)
                        .map(|f| f.default_window)
                }),
        }
    }
    pub fn volume(self, cancelled: impl Fn() -> bool) -> Result<Arc<Volume>, String> {
        if cancelled() {
            return Err("Cancelled".into());
        }
        match self {
            Self::Resident(frames) => Volume::prepare(frames, cancelled),
            Self::Indexed(series) => series.volume(cancelled),
        }
    }
}
