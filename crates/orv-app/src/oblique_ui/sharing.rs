// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use orv_core::scene::Reconstruction;
#[cfg(feature = "mcp")]
use orv_core::scene::SceneState;
impl ObliqueSession {
    pub(crate) fn shared_view(&self) -> Option<(crate::rendered::ViewSpec, bool)> {
        let snapshot = self.snapshot.as_ref()?;
        Some((
            crate::rendered::ViewSpec {
                view: self.view.clone(),
                point: None,
                reformat_side: snapshot.reformat_side,
                reconstruction: Some(if let Some(slab) = snapshot.image.slab {
                    Reconstruction::Slab(orv_core::slab::SlabPlane {
                        plane: snapshot.plane.clone(),
                        slab,
                    })
                } else {
                    Reconstruction::Oblique(snapshot.plane.clone())
                }),
                preferred_size: None,
            },
            snapshot.generation == self.worker.generation() && !snapshot.preview,
        ))
    }
    #[cfg(feature = "mcp")]
    pub(crate) fn apply_shared_view(&mut self, scene: &SceneState) {
        let (plane, slab) = match &scene.reconstruction {
            Some(Reconstruction::Oblique(plane)) => (plane, None),
            Some(Reconstruction::Slab(state)) => (&state.plane, Some(state.slab)),
            _ => return,
        };
        {
            self.slab = slab;
            self.base = Some(plane.clone());
            self.angles = [0.; 3];
            self.offset_mm = 0.;
            self.view = scene.view.clone();
            self.reformat_side = scene.reformat_side;
            self.annotations.clear();
            self.target = Some(plane.clone());
            self.worker
                .request_projection(plane.clone(), scene.reformat_side, slab, false);
        }
    }
}
