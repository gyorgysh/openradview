// SPDX-License-Identifier: GPL-3.0-or-later
//! Composition of the last completed view, shared by local export and approval.
use crate::workspace::{ViewMode, Workspace};
use orv_core::{ViewState, scene::Reconstruction};
#[derive(Clone)]
pub(crate) struct ViewSpec {
    pub view: ViewState,
    pub point: Option<[f64; 3]>,
    pub reformat_side: u32,
    pub reconstruction: Option<Reconstruction>,
    /// Initial review composition; existing grants retain their explicit output size.
    pub preferred_size: Option<[u32; 2]>,
}

impl Workspace {
    pub(crate) fn rendered_view(&self) -> Option<(ViewSpec, bool)> {
        self.study.as_ref()?.series.get(self.selected)?;
        match self.mode {
            ViewMode::Compare => None,
            ViewMode::Mpr => self.mpr.as_ref()?.shared_view(),
            ViewMode::Volume => self.volume.as_ref()?.shared_view(),
            ViewMode::Oblique => self.oblique.as_ref()?.shared_view(),
            ViewMode::Acquired => {
                let (index, _) = self.current_image()?;
                let mut view = self
                    .last_image_view
                    .clone()
                    .unwrap_or_else(|| self.view.clone());
                if self.last_image_view.is_none() {
                    view.slice = index as u32;
                }
                Some((
                    ViewSpec {
                        view,
                        reconstruction: None,
                        preferred_size: None,
                        point: None,
                        reformat_side: {
                            #[cfg(feature = "mcp")]
                            {
                                self.sharing.acquired_side()
                            }
                            #[cfg(not(feature = "mcp"))]
                            {
                                512
                            }
                        },
                    },
                    index == self.view.slice as usize
                        && self
                            .last_image_view
                            .as_ref()
                            .is_none_or(|v| v == &self.view),
                ))
            }
        }
    }
}

pub(crate) fn picture_size(size: eframe::egui::Vec2) -> [u32; 2] {
    let scale = (1024. / size.x.max(size.y).max(1.)).min(1.);
    [size.x, size.y].map(|v| (v * scale).round().clamp(1., 1024.) as u32)
}
