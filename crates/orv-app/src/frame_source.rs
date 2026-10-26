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
    pub fn volume(self, cancelled: impl Fn() -> bool) -> Result<Volume, String> {
        if cancelled() {
            return Err("Cancelled".into());
        }
        let frames = match self {
            Self::Resident(frames) => frames,
            Self::Indexed(series) => series.volume_frames(&cancelled).map_err(|e| if e==orv_core::load::LoadError::Limit {
                "This series exceeds the 512 MiB decoded volume budget. Its images remain available in 2D.".into()
            } else { e.to_string() })?,
        };
        if cancelled() {
            return Err("Cancelled".into());
        }
        Volume::new(frames).map_err(|e| e.to_string())
    }
}
