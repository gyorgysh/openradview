// SPDX-License-Identifier: GPL-3.0-or-later
//! Worker-only region probes against the exact completed source-section scene.
use super::*;
use crate::surface_pick::{
    MIN_PICK_TRANSMITTANCE, PickScope, SurfaceProbeKind, f32_at, half, matches_hit, ray_error,
    u32_at,
};
use orv_core::{
    mask::MaskOwner,
    surface::{InterfaceHit, InterfaceRay, InterfaceRayError},
};

mod batch;
pub use batch::{SectionProbeBatch, SectionProbeSample};
mod geometry;
mod gpu;
pub(super) use gpu::Pipeline;

/// Capture the originating scope with the image job. Never attach a new scope
/// to a retained old front when returning to an earlier camera or tool.
#[derive(Clone)]
pub struct SectionPickFrame {
    image: Arc<SectionImage>,
    scope: Arc<PickScope>,
}
impl SectionPickFrame {
    pub fn new(image: Arc<SectionImage>, scope: Arc<PickScope>) -> Self {
        Self { image, scope }
    }
    pub fn image(&self) -> &Arc<SectionImage> {
        &self.image
    }
    pub fn scope(&self) -> &Arc<PickScope> {
        &self.scope
    }
}
/// Live desired state, including exact plane and surface owners. The caller must
/// also compare its current CPU request before using a previously prepared scene.
#[derive(Clone)]
pub struct SectionPickState {
    pub scope: Arc<PickScope>,
    pub map: MaskOwner,
    pub number: u16,
    pub scene: SectionScene,
}
impl SectionPickState {
    /// Only for an already-current prepared scene. A retained old front alone
    /// cannot prove that the current CPU request or display controls match it.
    pub fn from_frame(frame: &SectionPickFrame) -> Result<Self, &'static str> {
        let scene = frame.image.scene();
        let surface = scene
            .surface
            .as_ref()
            .ok_or("No region in the completed scene")?;
        let cpu = surface.image.request().surface.surface();
        Ok(Self {
            scope: frame.scope.clone(),
            map: cpu.owner().clone(),
            number: cpu.key().number,
            scene: scene.clone(),
        })
    }
    pub fn matches(&self, frame: &SectionPickFrame) -> bool {
        let Some(surface) = &frame.image.scene.surface else {
            return false;
        };
        let Ok(view) = self.map.view(self.number) else {
            return false;
        };
        self.scope.is_active()
            && frame.scope.is_active()
            && Arc::ptr_eq(&self.scope, &frame.scope)
            && self.scene.same(frame.image.scene())
            && std::ptr::eq(view.source(), frame.image.source.volume().as_ref())
            && view.key() == *surface.image.key()
            && view.revision() == surface.image.revision()
    }
}
/// A local, read-only result. Neither this type nor its frame authorizes label
/// mutation, export or disclosure. Revalidate before displaying or using it.
pub struct SectionProbe {
    frame: SectionPickFrame,
    pixel: [u32; 2],
    kind: SurfaceProbeKind,
    hit: Option<InterfaceHit>,
    transmittance: f32,
    _metadata: Reservation,
}
impl SectionProbe {
    pub fn kind(&self) -> SurfaceProbeKind {
        self.kind
    }
    pub fn hit(&self) -> Option<&InterfaceHit> {
        self.hit.as_ref()
    }
    pub fn pixel(&self) -> [u32; 2] {
        self.pixel
    }
    /// Source transmission before the first displayed opaque actor. If a plane
    /// covers the region, this is transmission to that plane, not to the region.
    pub fn foreground_transmittance(&self) -> f32 {
        self.transmittance
    }
    pub fn is_current(&self, current: &SectionPickState, pixel: [u32; 2]) -> bool {
        self.pixel == pixel && current.matches(&self.frame)
    }
}
impl SectionRenderer {
    /// Worker-only. Uses the original image ray and reads 56 bytes. The optional
    /// pipeline is prepared on first use; a probe failure does not disable display.
    pub fn probe(
        &mut self,
        frame: &SectionPickFrame,
        current: &SectionPickState,
        pixel: [u32; 2],
        cancelled: impl Fn() -> bool,
    ) -> Result<SectionProbe, &'static str> {
        let obsolete = || cancelled() || !current.scope.is_active();
        if obsolete() {
            return Err("Cancelled");
        }
        if !current.matches(frame) {
            return Err("The completed sections are no longer the current view");
        }
        if !Arc::ptr_eq(&self.source, frame.image.source()) {
            return Err("The section probe and GPU source differ");
        }
        let scene = frame.image.scene();
        if (0..2).any(|a| pixel[a] >= scene.size[a]) {
            return Err("The section probe pixel is outside the completed image");
        }
        let metadata = MemoryBudget::volumes()
            .reserve(std::mem::size_of::<SectionProbe>() as u64 + 56)
            .ok_or("Not enough memory for the local section probe")?;
        if self.probe.is_none() {
            self.probe = Some(Pipeline::new(&self.source)?);
        }
        let bytes = self
            .probe
            .as_ref()
            .ok_or("Section probe pipeline unavailable")?
            .read(self, frame, pixel, &obsolete)?;
        let sample = self.sample(frame, current, pixel, &bytes, &obsolete)?;
        if obsolete() || !current.matches(frame) {
            return Err("Cancelled");
        }
        Ok(SectionProbe {
            frame: frame.clone(),
            pixel,
            kind: sample.kind,
            hit: sample.hit,
            transmittance: sample.transmittance,
            _metadata: metadata,
        })
    }
    fn sample(
        &self,
        frame: &SectionPickFrame,
        current: &SectionPickState,
        pixel: [u32; 2],
        bytes: &[u8; 56],
        obsolete: &impl Fn() -> bool,
    ) -> Result<SectionProbeSample, &'static str> {
        let scene = frame.image.scene();
        if u32_at(bytes, 20)? != 0 {
            return Err("Section probe arithmetic could not resolve this pixel");
        }
        let cells = [
            u32_at(bytes, 0)?,
            u32_at(bytes, 4)?,
            u32_at(bytes, 8)?,
            u32_at(bytes, 12)?,
        ];
        let depth = f32_at(bytes, 16)?;
        let radiance = f32_at(bytes, 32)?;
        let transmittance = f32_at(bytes, 36)?;
        let actor_depth = f32_at(bytes, 40)?;
        let actor = f32_at(bytes, 44)?;
        if !depth.is_finite()
            || !radiance.is_finite()
            || !(0. ..=1.00001).contains(&radiance)
            || !transmittance.is_finite()
            || !(0. ..=1.).contains(&transmittance)
            || !actor_depth.is_finite()
            || ![0., 1., 2., 3., 4.].contains(&actor)
        {
            return Err("The section probe returned invalid values");
        }
        for a in 0..4 {
            let offset = 48 + a * 2;
            let value = half(u16::from_le_bytes([bytes[offset], bytes[offset + 1]]))
                .ok_or("The completed section pixel is invalid")?;
            let expected = if a == 3 { 1. } else { radiance };
            if (value - expected).abs() > 0.0006 {
                return Err("The section probe and completed pixel differ");
            }
        }
        let source = self.source.volume();
        let camera = crate::physical_camera::centred_frame(
            &scene.volume.camera,
            &scene.navigation,
            scene.size,
            source.center(),
        )?;
        let (relative, direction) = camera.ray(std::array::from_fn(|a| {
            (f64::from(pixel[a]) + 0.5) / f64::from(scene.size[a])
        }));
        let origin = std::array::from_fn(|a| source.center()[a] + relative[a]);
        let view = current.map.view(current.number)?;
        let geometry = InterfaceRay::new(
            view,
            origin,
            direction,
            [-1e30, 1e30],
            scene.volume.clip.as_ref(),
        )
        .map_err(ray_error)?
        .first_hit(obsolete);
        let (kind, hit) = match geometry {
            Err(InterfaceRayError::Ambiguous) => (SurfaceProbeKind::Ambiguous, None),
            Err(error) => return Err(ray_error(error)),
            Ok(hit) => {
                let agrees = match &hit {
                    Some(hit) => matches_hit(hit, cells, depth, source.center(), direction, source),
                    None => cells == [0; 4] && depth == 0.,
                };
                let selected = geometry::nearest(scene, origin, direction, hit.as_ref())?;
                if !agrees
                    || selected.0 != actor as u32
                    || !geometry::depth_matches(selected.1, actor_depth, source, direction)
                {
                    (SurfaceProbeKind::Ambiguous, None)
                } else if hit.is_none() {
                    (SurfaceProbeKind::Empty, None)
                } else if selected.0 != 1 || transmittance < MIN_PICK_TRANSMITTANCE {
                    (SurfaceProbeKind::Obscured, None)
                } else {
                    (SurfaceProbeKind::Hit, hit)
                }
            }
        };
        Ok(SectionProbeSample {
            pixel,
            kind,
            hit,
            transmittance,
        })
    }
}
