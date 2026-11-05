// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use orv_render::{
    label_source::GpuLabels,
    mask_input::MaskInput,
    mask_projection::{GpuMaskProjector, MaskImage, MaskProjection},
    opaque_material::{OpaqueMaterialRenderer, OpaqueMaterialScene},
    segment_source::GpuSegment,
};

pub(super) struct Cells {
    source: Arc<GpuVolumeSource>,
    projector: GpuMaskProjector,
    renderer: OpaqueMaterialRenderer,
    cached: Option<MaskInput>,
    mask: Option<Arc<MaskImage>>,
    material: Option<Arc<orv_core::volume_material::VolumeMaterial>>,
}
impl Cells {
    fn new(source: Arc<GpuVolumeSource>, format: wgpu::TextureFormat) -> Result<Self, String> {
        Ok(Self {
            projector: GpuMaskProjector::new(source.clone()).map_err(str::to_owned)?,
            renderer: OpaqueMaterialRenderer::new(source.clone(), format).map_err(str::to_owned)?,
            source,
            cached: None,
            mask: None,
            material: None,
        })
    }
    fn render(
        &mut self,
        input: &Request,
        cancel: &dyn Fn() -> bool,
    ) -> Result<Arc<OpaqueMaterialImage>, String> {
        if !Arc::ptr_eq(&input.key.selection.map.source(), self.source.volume()) {
            return Err("Region and volume sources differ".into());
        }
        // Data upload is independent of camera motion. Keep warming this exact
        // immutable map while view generations change, or interaction can starve it.
        let data_cancel = || input.data_cancel.load(Ordering::Acquire);
        let cached = match &input.key.selection.map {
            crate::labels::RegionMap::Manual(labels) => {
                let previous = match &self.cached {
                    Some(MaskInput::Manual(v)) => Some(v.clone()),
                    _ => None,
                };
                MaskInput::Manual(
                    GpuLabels::upload(self.source.clone(), labels.clone(), previous, data_cancel)
                        .map_err(str::to_owned)?
                        .0,
                )
            }
            crate::labels::RegionMap::Supplied(segments) => {
                let previous = match &self.cached {
                    Some(MaskInput::Supplied(v)) => Some(v.clone()),
                    _ => None,
                };
                MaskInput::Supplied(
                    GpuSegment::upload(
                        self.source.clone(),
                        segments.clone(),
                        input.key.selection.number,
                        previous,
                        data_cancel,
                    )
                    .map_err(str::to_owned)?
                    .0,
                )
            }
        };
        self.cached = Some(cached.clone());
        if cancel() {
            return Err("Cancelled".into());
        }
        let request = MaskProjection {
            input: cached,
            number: input.key.selection.number,
            camera: input.key.state.camera.clone(),
            display: input.key.display.clone(),
            clip: input.key.state.clip.clone(),
            size: input.size,
        };
        let reusable = self.mask.as_ref().is_some_and(|v| {
            v.key() == input.key.selection.key
                && v.revision() == &input.key.selection.revision
                && v.request().camera == request.camera
                && v.request().display == request.display
                && v.request().clip == request.clip
                && v.request().size == request.size
        });
        if !reusable {
            self.mask = None;
            let step = if input.refined { 16 } else { input.size[1] };
            for y in (0..input.size[1]).step_by(step as usize) {
                if cancel() {
                    return Err("Cancelled".into());
                }
                let mut encoder = self
                    .source
                    .device()
                    .create_command_encoder(&Default::default());
                let image = self
                    .projector
                    .encode_rows(&mut encoder, &request, 0, y..(y + step).min(input.size[1]))
                    .map_err(str::to_owned)?;
                self.source.queue().submit([encoder.finish()]);
                if image.is_some() {
                    self.mask = image;
                }
                if input.refined {
                    self.source.wait_submitted(cancel).map_err(str::to_owned)?;
                }
            }
        }
        let mask = self
            .mask
            .clone()
            .ok_or("Region projection did not complete")?;
        if self
            .material
            .as_ref()
            .is_none_or(|v| **v != input.key.style.material)
        {
            self.material = Some(Arc::new(input.key.style.material.clone()));
        }
        let mut state = input.key.state.clone();
        state.quality = if input.refined {
            Quality::Refined
        } else {
            Quality::Interactive
        };
        let scene = OpaqueMaterialScene {
            mask,
            navigation: input.key.display.clone(),
            volume: state,
            material: self.material.clone().ok_or("Region material unavailable")?,
            lighting: input.key.style.lighting,
            mask_luminance: input.key.luminance,
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
        complete.ok_or_else(|| "Region composition did not complete".into())
    }
}
pub(super) enum Engine {
    Cells(Box<Cells>),
    Faces(Box<super::surface::Engine>),
}
impl Engine {
    fn new(
        source: Arc<GpuVolumeSource>,
        format: wgpu::TextureFormat,
        geometry: Geometry,
    ) -> Result<Self, String> {
        match geometry {
            Geometry::Cells => {
                Cells::new(source, format).map(|engine| Self::Cells(Box::new(engine)))
            }
            Geometry::Faces => super::surface::Engine::new(source, format)
                .map(|engine| Self::Faces(Box::new(engine))),
        }
    }
    fn matches(&self, geometry: Geometry) -> bool {
        matches!(
            (self, geometry),
            (Self::Cells(_), Geometry::Cells) | (Self::Faces(_), Geometry::Faces)
        )
    }
    fn render(&mut self, input: &Request, cancel: &dyn Fn() -> bool) -> Result<Arc<Image>, String> {
        match self {
            Self::Cells(engine) => engine
                .render(input, cancel)
                .map(|image| Arc::new(Image::Cells(image))),
            Self::Faces(engine) => engine
                .render(input, cancel)
                .map(|image| Arc::new(Image::Faces(image))),
        }
    }
    fn cancel(&mut self) {
        match self {
            Self::Cells(engine) => {
                let _ = engine.projector.discard_pending(0);
                let _ = engine.renderer.cancel(0);
            }
            Self::Faces(engine) => engine.cancel(),
        }
    }
}
pub(super) fn run(input: Request, cancel: &dyn Fn() -> bool) -> Result<Completed, String> {
    if cancel() {
        return Err("Cancelled".into());
    }
    let mut engine = input
        .engine
        .lock()
        .map_err(|_| "Region renderer unavailable; reopen 3D")?;
    if engine
        .as_ref()
        .is_none_or(|engine| !engine.matches(input.key.geometry))
    {
        *engine = None;
        *engine = Some(Engine::new(
            input.source.clone(),
            input.format,
            input.key.geometry,
        )?);
    }
    let engine = engine.as_mut().ok_or("Region renderer unavailable")?;
    let result = engine.render(&input, cancel);
    if result.is_err() {
        engine.cancel();
    }
    let image = result?;
    if cancel() {
        return Err("Cancelled".into());
    }
    Ok(Completed {
        key: input.key,
        refined: input.refined,
        image,
        scope: input.scope,
    })
}
