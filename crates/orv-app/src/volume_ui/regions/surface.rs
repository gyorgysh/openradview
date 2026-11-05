// SPDX-License-Identifier: GPL-3.0-or-later
//! Source-bound cell faces and ordered optics, prepared on the region worker.
use super::*;
use orv_core::surface::{CellSurface, SurfaceLimits};
use orv_render::{
    material_volume::Lighting,
    opaque_surface::{OpaqueSurfaceRenderer, OpaqueSurfaceScene},
    surface_projection::{GpuSurfaceProjector, SurfaceImage, SurfaceProjection},
    surface_source::GpuCellSurface,
};

pub(super) struct Engine {
    source: Arc<GpuVolumeSource>,
    projector: GpuSurfaceProjector,
    renderer: OpaqueSurfaceRenderer,
    uploaded: Option<Arc<GpuCellSurface>>,
    projected: Option<Arc<SurfaceImage>>,
    material: Option<Arc<orv_core::volume_material::VolumeMaterial>>,
}
impl Engine {
    pub fn new(source: Arc<GpuVolumeSource>, format: wgpu::TextureFormat) -> Result<Self, String> {
        Ok(Self {
            projector: GpuSurfaceProjector::new(source.clone()).map_err(str::to_owned)?,
            renderer: OpaqueSurfaceRenderer::new(source.clone(), format).map_err(str::to_owned)?,
            source,
            uploaded: None,
            projected: None,
            material: None,
        })
    }
    pub fn cancel(&mut self) {
        let _ = self.renderer.cancel(0);
    }
    pub fn render(
        &mut self,
        input: &Request,
        cancel: &dyn Fn() -> bool,
    ) -> Result<Arc<OpaqueSurfaceImage>, String> {
        if !Arc::ptr_eq(&input.key.selection.map.source(), self.source.volume()) {
            return Err("Region and volume sources differ".into());
        }
        let data_cancel = || input.data_cancel.load(Ordering::Acquire);
        let (surface, _) = CellSurface::build(
            input.key.selection.map.clone(),
            input.key.selection.number,
            self.uploaded.as_ref().map(|gpu| gpu.surface()),
            SurfaceLimits::default(),
            data_cancel,
        )
        .map_err(|error| error.to_string())?;
        let (uploaded, _) = GpuCellSurface::upload(
            self.source.clone(),
            surface,
            self.uploaded.clone(),
            data_cancel,
        )
        .map_err(str::to_owned)?;
        self.uploaded = Some(uploaded.clone());
        if cancel() {
            return Err("Cancelled".into());
        }
        let request = SurfaceProjection {
            surface: uploaded,
            camera: input.key.state.camera.clone(),
            display: input.key.display.clone(),
            clip: input.key.state.clip.clone(),
            size: input.size,
        };
        let reusable = self.projected.as_ref().is_some_and(|image| {
            let old = image.request();
            Arc::ptr_eq(&old.surface, &request.surface)
                && old.camera == request.camera
                && old.display == request.display
                && old.clip == request.clip
                && old.size == request.size
        });
        if !reusable {
            self.projected = None;
            self.projected = Some(
                self.projector
                    .render(request, cancel)
                    .map_err(str::to_owned)?,
            );
        }
        if self
            .material
            .as_ref()
            .is_none_or(|material| **material != input.key.style.material)
        {
            self.material = Some(Arc::new(input.key.style.material.clone()));
        }
        let mut state = input.key.state.clone();
        state.quality = if input.refined {
            Quality::Refined
        } else {
            Quality::Interactive
        };
        let scene = OpaqueSurfaceScene {
            surface: self
                .projected
                .clone()
                .ok_or("Cell-face projection did not complete")?,
            navigation: input.key.display.clone(),
            volume: state,
            material: self
                .material
                .clone()
                .ok_or("Cell-face material unavailable")?,
            lighting: input.key.style.lighting,
            surface_lighting: if input.key.surface_lighting {
                Lighting::HEADLIGHT
            } else {
                Lighting::UNLIT
            },
            surface_luminance: input.key.luminance,
        };
        let step = if input.refined { 16 } else { input.size[1] };
        let mut complete = None;
        for y in (0..input.size[1]).step_by(step as usize) {
            complete = self
                .renderer
                .render_rows(&scene, 0, y..(y + step).min(input.size[1]), cancel)
                .map_err(str::to_owned)?;
            if input.refined && complete.is_none() {
                self.source.wait_submitted(cancel).map_err(str::to_owned)?;
            }
        }
        complete.ok_or_else(|| "Cell-face composition did not complete".into())
    }
}
