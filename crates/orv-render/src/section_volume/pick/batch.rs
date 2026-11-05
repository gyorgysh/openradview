// SPDX-License-Identifier: GPL-3.0-or-later
//! Complete bounded rectangles of read-only, original-raster visibility proofs.
use super::*;

pub struct SectionProbeSample {
    pub(super) pixel: [u32; 2],
    pub(super) kind: SurfaceProbeKind,
    pub(super) hit: Option<InterfaceHit>,
    pub(super) transmittance: f32,
}
impl SectionProbeSample {
    pub fn pixel(&self) -> [u32; 2] {
        self.pixel
    }
    pub fn kind(&self) -> SurfaceProbeKind {
        self.kind
    }
    pub fn hit(&self) -> Option<&InterfaceHit> {
        self.hit.as_ref()
    }
    pub fn foreground_transmittance(&self) -> f32 {
        self.transmittance
    }
}
/// Every requested pixel has a checked outcome, including empty/obscured/
/// ambiguous pixels. The batch carries no mutation or disclosure permission.
/// Revalidate the owning batch before using any of its samples.
pub struct SectionProbeBatch {
    frame: SectionPickFrame,
    region: [[u32; 2]; 2],
    samples: Vec<SectionProbeSample>,
    _memory: Reservation,
}
impl SectionProbeBatch {
    pub fn region(&self) -> [[u32; 2]; 2] {
        self.region
    }
    /// Row-major samples keep their original completed-image pixel coordinates.
    pub fn samples(&self) -> &[SectionProbeSample] {
        &self.samples
    }
    pub fn is_current(&self, current: &SectionPickState, region: [[u32; 2]; 2]) -> bool {
        self.region == region && current.matches(&self.frame)
    }
}
impl SectionRenderer {
    /// Worker-only; at most 4096 pixels and 16 rows at the original resolution.
    /// All GPU/color/f64 checks finish before publication. Cancellation, stale
    /// scope, arithmetic or allocation failure exposes no partial batch.
    pub fn probe_region(
        &mut self,
        frame: &SectionPickFrame,
        current: &SectionPickState,
        region: [[u32; 2]; 2],
        cancelled: impl Fn() -> bool,
    ) -> Result<SectionProbeBatch, &'static str> {
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
        let count = u64::from(region[1][0]) * u64::from(region[1][1]);
        if region[1].contains(&0)
            || region[1][1] > 16
            || count > 4096
            || (0..2).any(|a| {
                region[0][a]
                    .checked_add(region[1][a])
                    .is_none_or(|end| end > current.scene.size[a])
            })
        {
            return Err("Section probe batch exceeds the image or working bounds");
        }
        let count = count as usize;
        let memory = MemoryBudget::volumes()
            .reserve(
                (std::mem::size_of::<SectionProbeBatch>()
                    + count * std::mem::size_of::<SectionProbeSample>()) as u64,
            )
            .ok_or("Not enough memory for the section probe batch")?;
        let mut samples = Vec::new();
        samples
            .try_reserve_exact(count)
            .map_err(|_| "Not enough memory for section probe outcomes")?;
        if self.probe.is_none() {
            self.probe = Some(Pipeline::new(&self.source)?);
        }
        self.probe
            .as_ref()
            .ok_or("Section probe batch pipeline unavailable")?
            .read_region(self, frame, region, &obsolete, |data, packing| {
                for y in 0..region[1][1] {
                    for x in 0..region[1][0] {
                        if obsolete() {
                            return Err("Cancelled");
                        }
                        let pixel = [region[0][0] + x, region[0][1] + y];
                        let packet = packing.packet(data, [x, y])?;
                        samples.push(self.sample(frame, current, pixel, &packet, &obsolete)?);
                    }
                }
                Ok(())
            })?;
        if obsolete() || !current.matches(frame) {
            return Err("Cancelled");
        }
        Ok(SectionProbeBatch {
            frame: frame.clone(),
            region,
            samples,
            _memory: memory,
        })
    }
}
