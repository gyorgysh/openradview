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
    faces: Faces,
    renderer: OpaqueSurfaceRenderer,
    material: Option<Arc<orv_core::volume_material::VolumeMaterial>>,
}
#[derive(Clone)]
pub(in crate::volume_ui) struct Faces {
    source: Arc<GpuVolumeSource>,
    // Projection resources are immutable; each render allocates its own targets.
    // Cloning a cache shares the base revision without replacing its owners.
    projector: Arc<GpuSurfaceProjector>,
    uploaded: Option<Arc<GpuCellSurface>>,
    projected: Option<Arc<SurfaceImage>>,
}
impl Faces {
    pub fn new(source: Arc<GpuVolumeSource>) -> Result<Self, String> {
        Ok(Self {
            projector: Arc::new(GpuSurfaceProjector::new(source.clone()).map_err(str::to_owned)?),
            source,
            uploaded: None,
            projected: None,
        })
    }
    pub fn project(
        &mut self,
        selection: &RegionSelection,
        display: &ViewState,
        state: &VolumeView,
        size: [u32; 2],
        data_cancel: &dyn Fn() -> bool,
        cancel: &dyn Fn() -> bool,
    ) -> Result<Arc<SurfaceImage>, String> {
        if !Arc::ptr_eq(&selection.map.source(), self.source.volume()) {
            return Err("Region and volume sources differ".into());
        }
        let (surface, _) = CellSurface::build(
            selection.map.clone(),
            selection.number,
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
            camera: state.camera.clone(),
            display: display.clone(),
            clip: state.clip.clone(),
            size,
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
        self.projected
            .clone()
            .ok_or_else(|| "Cell-face projection did not complete".into())
    }
}
impl Engine {
    pub fn new(source: Arc<GpuVolumeSource>, format: wgpu::TextureFormat) -> Result<Self, String> {
        Ok(Self {
            faces: Faces::new(source.clone())?,
            renderer: OpaqueSurfaceRenderer::new(source, format).map_err(str::to_owned)?,
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
        let data_cancel = || input.data_cancel.load(Ordering::Acquire);
        let surface = self.faces.project(
            &input.key.selection,
            &input.key.display,
            &input.key.state,
            input.size,
            &data_cancel,
            cancel,
        )?;
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
            surface,
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
                self.faces
                    .source
                    .wait_submitted(cancel)
                    .map_err(str::to_owned)?;
            }
        }
        complete.ok_or_else(|| "Cell-face composition did not complete".into())
    }
}
