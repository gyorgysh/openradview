// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use orv_core::scene::Reconstruction;
#[cfg(feature = "mcp")]
use orv_core::scene::SceneState;
impl VolumeSession {
    pub(crate) fn shared_view(&self) -> Option<(crate::rendered::ViewSpec, bool)> {
        let state = self.state.as_ref()?;
        let gpu = self.gpu.as_ref()?.try_lock().ok()?;
        Some((
            crate::rendered::ViewSpec {
                view: self.display.clone(),
                point: None,
                reformat_side: 2,
                reconstruction: Some(Reconstruction::Volume(state.clone())),
                preferred_size: gpu.preferred_snapshot_size(),
            },
            !self.needs_fit && gpu.completed(&self.display, state),
        ))
    }
    #[cfg(feature = "mcp")]
    pub(crate) fn apply_shared_view(&mut self, scene: &SceneState) {
        if let Some(Reconstruction::Volume(state)) = &scene.reconstruction {
            self.display = scene.view.clone();
            self.state = Some(state.clone());
            self.needs_fit = false;
        }
    }
}
