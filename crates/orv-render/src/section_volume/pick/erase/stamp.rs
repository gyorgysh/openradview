// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use orv_core::{
    labels::{BrushAction, BrushShape, MAX_PHYSICAL_BRUSH_RADIUS_MM},
    volume::Volume,
};

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| a * b).sum()
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn length(v: [f64; 3]) -> f64 {
    v[0].hypot(v[1]).hypot(v[2])
}
/// Bound every ray that can touch a selected cell whose centre meets the sphere.
/// A face can be farther from its centre than the brush radius; account for the
/// full affine half-cell, including reflected/sheared bases and grid-edge faces.
fn region(
    source: &Volume,
    scene: &SectionScene,
    point: [f64; 3],
    radius_mm: f64,
) -> Result<[[u32; 2]; 2], &'static str> {
    let reference = source.center();
    let frame = crate::physical_camera::centred_frame(
        &scene.volume.camera,
        &scene.navigation,
        scene.size,
        reference,
    )?;
    let spans = [length(frame.right_mm), length(frame.down_mm)];
    if spans.iter().any(|s| !s.is_finite() || *s <= 0.) {
        return Err("The brush viewport spans cannot be represented");
    }
    let right = frame.right_mm.map(|v| v / spans[0]);
    let down = frame.down_mm.map(|v| v / spans[1]);
    let x = cross(down, frame.direction_unit);
    let y = cross(frame.direction_unit, right);
    let determinant = dot(right, x);
    if !determinant.is_finite() || !(0.5..=2.).contains(&determinant.abs()) {
        return Err("The brush viewport basis cannot be bounded");
    }
    let inverse = [x.map(|v| v / determinant), y.map(|v| v / determinant)];
    let offset = std::array::from_fn(|a| point[a] - reference[a] - frame.origin_mm[a]);
    let mut half_cell = 0f64;
    for corner in 0..8 {
        let p = source.voxel_vector_to_world(std::array::from_fn(|a| {
            if corner & (1 << a) == 0 { -0.5 } else { 0.5 }
        }));
        let extent = length(p);
        if !extent.is_finite() {
            return Err("The source half-cell extent cannot be represented");
        }
        half_cell = half_cell.max(extent);
    }
    let radius = radius_mm + half_cell;
    // Cross/dot operations above use scaled unit bases and a determinant bounded
    // away from zero. Bound their rounding plus the absolute-world subtraction;
    // refuse an unresolved zoom instead of pruning away a possible source cell.
    let magnitude = point.into_iter().map(f64::abs).sum::<f64>()
        + reference.into_iter().map(f64::abs).sum::<f64>()
        + frame.origin_mm.into_iter().map(f64::abs).sum::<f64>()
        + radius
        + 1.;
    let error_mm = 128. * f64::EPSILON * magnitude;
    let mut minimum = [0; 2];
    let mut maximum = [0; 2];
    for a in 0..2 {
        let scale = f64::from(scene.size[a]) / spans[a];
        let norm = length(inverse[a]);
        let centre = (0.5 + dot(offset, inverse[a]) / spans[a]) * f64::from(scene.size[a]);
        let extent = radius * norm * scale;
        let error = error_mm * norm * scale;
        if !centre.is_finite() || !extent.is_finite() || !error.is_finite() || error > 0.25 {
            return Err("The brush footprint cannot be bounded at this zoom");
        }
        // Include pixel-centre rounding on both edges. This expands only the
        // proof rectangle; it never shifts a ray or widens the physical brush.
        minimum[a] = (centre - extent - error - 2.)
            .floor()
            .clamp(0., f64::from(scene.size[a])) as u32;
        maximum[a] = (centre + extent + error + 2.)
            .ceil()
            .clamp(0., f64::from(scene.size[a])) as u32;
    }
    let size = std::array::from_fn(|a| maximum[a] - minimum[a]);
    if size.contains(&0) {
        return Err("The brush footprint is outside the completed image");
    }
    if u64::from(size[0]) * u64::from(size[1]) > MAX_ERASE_FACES as u64 {
        return Err("The brush covers too many image pixels; choose a smaller radius or zoom out");
    }
    Ok([minimum, size])
}

impl SectionRenderer {
    /// Worker-only one-click visible-layer erase stamp, returning its private
    /// proposal and original-raster proof rectangle. A checked Hit anchors the
    /// sphere at that source-cell centre. Resolution is never reduced to meet
    /// the 65,536-sample cap. Supplied/covered/ambiguous anchors refuse the stamp.
    /// This does not capture a continuous gesture or publish the prepared edit.
    pub fn propose_erase_stamp(
        &mut self,
        frame: &SectionPickFrame,
        current: &SectionPickState,
        pixel: [u32; 2],
        settings: &PhysicalBrush,
        cancelled: impl Fn() -> bool,
    ) -> Result<(SectionFaceErase, [[u32; 2]; 2]), &'static str> {
        let obsolete = || cancelled() || !current.scope.is_active();
        if obsolete() {
            return Err("Cancelled");
        }
        if !current.matches(frame) {
            return Err("The completed sections are no longer the current view");
        }
        if !matches!(current.map, MaskOwner::Manual(_)) {
            return Err("Supplied masks are read-only");
        }
        if u16::from(settings.label) != current.number
            || settings.label == 0
            || settings.protected_ids[usize::from(settings.label)]
            || settings.action != BrushAction::Erase
            || settings.shape != BrushShape::Sphere
            || settings.clip != current.scene.volume.clip
        {
            return Err("Choose an unprotected manual ID, erase sphere and matching view clip");
        }
        if !settings.radius_mm.is_finite()
            || !(1e-6..=MAX_PHYSICAL_BRUSH_RADIUS_MM).contains(&settings.radius_mm)
        {
            return Err("Physical brush radius must be from 0.000001 to 256 mm");
        }
        let anchor = self.probe(frame, current, pixel, obsolete)?;
        if anchor.kind() != SurfaceProbeKind::Hit {
            return Err(match anchor.kind() {
                SurfaceProbeKind::Empty => "Choose a visible known cell face for the erase stamp",
                SurfaceProbeKind::Obscured => {
                    "The erase anchor is covered by a plane or foreground source"
                }
                SurfaceProbeKind::Ambiguous => {
                    "The erase anchor is ambiguous; choose another point"
                }
                SurfaceProbeKind::Hit => "No checked erase anchor",
            });
        }
        let cell = anchor.hit().ok_or("No checked erase anchor")?.face.cell();
        let point = self.source.volume().voxel_to_world(cell.map(f64::from));
        let bounds = region(
            self.source.volume(),
            &current.scene,
            point,
            settings.radius_mm,
        )?;
        let count = bounds[1][0].div_ceil(256) as usize * bounds[1][1].div_ceil(16) as usize;
        let _directory = MemoryBudget::volumes()
            .reserve((count * std::mem::size_of::<SectionProbeBatch>()) as u64)
            .ok_or("Not enough memory for the erase stamp proof directory")?;
        let mut batches = Vec::new();
        batches
            .try_reserve_exact(count)
            .map_err(|_| "Not enough memory for the erase stamp proof directory")?;
        for y in (0..bounds[1][1]).step_by(16) {
            for x in (0..bounds[1][0]).step_by(256) {
                if obsolete() {
                    return Err("Cancelled");
                }
                batches.push(self.probe_region(
                    frame,
                    current,
                    [
                        [bounds[0][0] + x, bounds[0][1] + y],
                        [(bounds[1][0] - x).min(256), (bounds[1][1] - y).min(16)],
                    ],
                    obsolete,
                )?);
            }
        }
        let proposal = SectionFaceErase::prepare(current, &batches, settings, &[point], obsolete)?;
        if obsolete() || !current.matches(frame) {
            return Err("Cancelled");
        }
        Ok((proposal, bounds))
    }
}
