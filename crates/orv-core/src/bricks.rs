// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded source-cell brick layouts and prototype CPU extraction.
//!
//! Measured need: a generated 4096x512x256 f32 volume (2 GiB) is admitted on
//! the CPU but refused by Metal's 2048-voxel 3D texture side. Bricks split the
//! volume into textures within a device side limit. Each stores its interior
//! cells plus the declared interpolation/gradient halo. Global sample positions
//! stay unchanged across page boundaries, including missing samples. The dense
//! CPU volume stays resident:
//! this changes GPU residency only and adds no disk spill or second CPU copy.
use crate::{resources::MemoryBudget, volume::Volume};

/// Bound on brick count, keeping the page table small.
pub const MAX_BRICKS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrickStencil {
    Trilinear,
    /// Six clipped secants, including a positive rounding guard for f32 queries.
    CentralSecants,
}
impl BrickStencil {
    fn halo(self) -> [usize; 2] {
        match self {
            Self::Trilinear => [0, 1],
            // Just below a power-of-two cell boundary, f32 (q + 1) can round
            // to the next integer. Preserve even its zero-weight upper corner
            // so the GPU uses the identical eight-source-value arithmetic.
            Self::CentralSecants => [1, 3],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrickError {
    Limit,
    Color,
    Memory,
    Cancelled,
}

/// One brick: the voxel range it stores, including its halo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Brick {
    pub origin: [usize; 3],
    pub size: [usize; 3],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrickLayout {
    dimensions: [usize; 3],
    /// Interior cells per brick along each axis (stored side minus the halo).
    interior: [usize; 3],
    counts: [usize; 3],
    stencil: BrickStencil,
}
impl BrickLayout {
    /// `max_side` is the device's largest texture side, at least 3.
    pub fn plan(dimensions: [usize; 3], max_side: usize) -> Result<Self, BrickError> {
        if max_side < 3 {
            return Err(BrickError::Limit);
        }
        Self::make(dimensions, max_side - 1, BrickStencil::Trilinear)
    }
    /// Fixed core-cell pages for actual GPU residency, independent of the
    /// device's largest texture side. Stored sides include the declared stencil.
    pub fn with_stencil(
        dimensions: [usize; 3],
        cell_side: usize,
        stencil: BrickStencil,
    ) -> Result<Self, BrickError> {
        if ![32, 64, 128].contains(&cell_side) {
            return Err(BrickError::Limit);
        }
        Self::make(dimensions, cell_side, stencil)
    }
    fn make(
        dimensions: [usize; 3],
        side: usize,
        stencil: BrickStencil,
    ) -> Result<Self, BrickError> {
        if dimensions.iter().any(|d| *d < 2)
            || dimensions
                .into_iter()
                .try_fold(4u64, |n, d| n.checked_mul(d as u64))
                .is_none_or(|n| n > crate::resources::MAX_VOLUME_BYTES)
        {
            return Err(BrickError::Limit);
        }
        // A brick owns cells [k*n, (k+1)*n) and stores voxels up to (k+1)*n.
        let interior = [side; 3];
        let counts: [usize; 3] =
            std::array::from_fn(|axis| (dimensions[axis] - 1).div_ceil(interior[axis]));
        if counts
            .iter()
            .try_fold(1usize, |n, c| n.checked_mul(*c))
            .is_none_or(|n| n > MAX_BRICKS)
        {
            return Err(BrickError::Limit);
        }
        Ok(Self {
            dimensions,
            interior,
            counts,
            stencil,
        })
    }
    pub fn len(&self) -> usize {
        self.counts.iter().product()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn brick(&self, index: usize) -> Option<Brick> {
        let [lo, hi] = self.cells(index)?;
        let [before, after] = self.stencil.halo();
        let origin = lo.map(|v| v.saturating_sub(before));
        let size = std::array::from_fn(|a| {
            hi[a].saturating_add(after).min(self.dimensions[a]) - origin[a]
        });
        Some(Brick { origin, size })
    }
    pub fn dimensions(&self) -> [usize; 3] {
        self.dimensions
    }
    pub fn stencil(&self) -> BrickStencil {
        self.stencil
    }
    pub fn cell_side(&self) -> [usize; 3] {
        self.interior
    }
    /// Owned lower-corner interpolation cells, minimum and exclusive maximum.
    pub fn cells(&self, index: usize) -> Option<[[usize; 3]; 2]> {
        if index >= self.len() {
            return None;
        }
        let cell = [
            index % self.counts[0],
            index / self.counts[0] % self.counts[1],
            index / (self.counts[0] * self.counts[1]),
        ];
        let origin: [usize; 3] = std::array::from_fn(|axis| cell[axis] * self.interior[axis]);
        let maximum =
            std::array::from_fn(|a| (origin[a] + self.interior[a]).min(self.dimensions[a] - 1));
        Some([origin, maximum])
    }
    /// Page-table lookup: the brick owning the trilinear cell of voxel `v`.
    pub fn owner(&self, v: [f64; 3]) -> Option<usize> {
        if v.iter().enumerate().any(|(a, p)| {
            !p.is_finite() || *p < -1e-6 || *p > (self.dimensions[a] - 1) as f64 + 1e-6
        }) {
            return None;
        }
        let cell: [usize; 3] = std::array::from_fn(|axis| {
            let last_cell = self.dimensions[axis] - 2;
            (v[axis].floor().max(0.) as usize).min(last_cell) / self.interior[axis]
        });
        if cell.iter().zip(self.counts).any(|(c, n)| *c >= n) {
            return None;
        }
        Some(cell[0] + self.counts[0] * (cell[1] + self.counts[1] * cell[2]))
    }
    /// Voxels stored by all bricks, including halos.
    pub fn stored_voxels(&self) -> u64 {
        (0..self.len())
            .filter_map(|index| self.brick(index))
            .map(|brick| brick.size.iter().map(|s| *s as u64).product::<u64>())
            .sum()
    }
}

/// Brick-local values with an optional validity plane, as a texture upload would hold.
#[derive(Clone, Debug, PartialEq)]
pub struct BrickData {
    pub brick: Brick,
    pub values: Vec<f32>,
    pub valid: Option<Vec<u8>>,
}

/// Copy one brick. The transient copy is admitted against the GPU budget it
/// stands in for and released when dropped by the caller's upload path.
pub fn extract(
    volume: &Volume,
    brick: Brick,
    cancelled: &impl Fn() -> bool,
) -> Result<(BrickData, crate::resources::Reservation), BrickError> {
    if volume.is_color() {
        return Err(BrickError::Color);
    }
    let count = checked_brick(volume.dimensions(), brick).ok_or(BrickError::Limit)?;
    let missing = volume.has_missing_scalar();
    let bytes = count as u64 * if missing { 5 } else { 4 };
    let reservation = MemoryBudget::gpu()
        .reserve(bytes)
        .ok_or(BrickError::Memory)?;
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| BrickError::Memory)?;
    let mut valid = if missing {
        let mut valid = Vec::new();
        valid
            .try_reserve_exact(count)
            .map_err(|_| BrickError::Memory)?;
        Some(valid)
    } else {
        None
    };
    let width = volume.dimensions()[0];
    for z in brick.origin[2]..brick.origin[2] + brick.size[2] {
        if cancelled() {
            return Err(BrickError::Cancelled);
        }
        let frame = &volume.frames()[z];
        for y in brick.origin[1]..brick.origin[1] + brick.size[1] {
            if cancelled() {
                return Err(BrickError::Cancelled);
            }
            let row = y * width;
            for x in brick.origin[0]..brick.origin[0] + brick.size[0] {
                values.push(frame.pixels[row + x]);
                if let Some(valid) = &mut valid {
                    valid.push(u8::from(frame.has_sample(row + x)));
                }
            }
        }
    }
    Ok((
        BrickData {
            brick,
            values,
            valid,
        },
        reservation,
    ))
}

impl BrickData {
    /// Trilinear sample at a whole-volume voxel coordinate inside this brick's
    /// owned cells, with the same missing-sample rule as the dense sampler.
    pub fn sample(&self, v: [f64; 3], dimensions: [usize; 3]) -> Option<f32> {
        let count = checked_brick(dimensions, self.brick)?;
        if self.values.len() != count || self.valid.as_ref().is_some_and(|v| v.len() != count) {
            return None;
        }
        if v.iter().any(|p| !p.is_finite())
            || v.iter()
                .enumerate()
                .any(|(i, p)| *p < -1e-6 || *p > (dimensions[i] - 1) as f64 + 1e-6)
        {
            return None;
        }
        let v: [f64; 3] = std::array::from_fn(|i| v[i].clamp(0., (dimensions[i] - 1) as f64));
        let lo: [usize; 3] = std::array::from_fn(|i| v[i].floor() as usize);
        let hi: [usize; 3] = std::array::from_fn(|i| (lo[i] + 1).min(dimensions[i] - 1));
        let t: [f64; 3] = std::array::from_fn(|i| v[i] - lo[i] as f64);
        let [w, h, _] = self.brick.size;
        let local = |p: [usize; 3]| -> Option<usize> {
            let l: [usize; 3] = std::array::from_fn(|i| {
                p[i].checked_sub(self.brick.origin[i]).unwrap_or(usize::MAX)
            });
            (l.iter().zip(self.brick.size).all(|(c, s)| *c < s))
                .then(|| (l[2] * h + l[1]) * w + l[0])
        };
        let mut value = 0.;
        for (z, wz) in [(lo[2], 1. - t[2]), (hi[2], t[2])] {
            for (y, wy) in [(lo[1], 1. - t[1]), (hi[1], t[1])] {
                for (x, wx) in [(lo[0], 1. - t[0]), (hi[0], t[0])] {
                    let weight = wx * wy * wz;
                    if weight == 0. {
                        continue;
                    }
                    let index = local([x, y, z])?;
                    if self.valid.as_ref().is_some_and(|valid| valid[index] != 1)
                        || !self.values[index].is_finite()
                    {
                        return None;
                    }
                    value += f64::from(self.values[index]) * weight;
                }
            }
        }
        Some(value as f32)
    }
}

fn checked_brick(dimensions: [usize; 3], brick: Brick) -> Option<usize> {
    if dimensions.iter().any(|d| *d < 2)
        || dimensions
            .into_iter()
            .try_fold(4u64, |n, d| n.checked_mul(d as u64))
            .is_none_or(|n| n > crate::resources::MAX_VOLUME_BYTES)
    {
        return None;
    }
    if (0..3).any(|a| {
        brick.size[a] == 0
            || brick.origin[a]
                .checked_add(brick.size[a])
                .is_none_or(|n| n > dimensions[a])
    }) {
        return None;
    }
    brick.size.into_iter().try_fold(1usize, usize::checked_mul)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        WindowLevel,
        study::{Frame, Geometry},
    };
    use std::sync::Arc;

    fn volume(dimensions: [usize; 3], missing: bool) -> Arc<Volume> {
        let [width, height, depth] = dimensions;
        let frames = (0..depth)
            .map(|z| {
                let pixels: Vec<f32> = (0..width * height)
                    .map(|i| ((i * 7 + z * 13) % 31) as f32 - 0.25 * z as f32)
                    .collect();
                let validity = missing.then(|| {
                    (0..width * height)
                        .map(|i| u8::from((i + z) % 11 != 0))
                        .collect::<Vec<u8>>()
                        .into()
                });
                Arc::new(Frame {
                    memory: None,
                    presentation: None,
                    calibration: None,
                    projection: None,
                    sop: None,
                    slab: None,
                    scalar: None,
                    validity,
                    color: None,
                    nifti: None,
                    rgba: None,
                    width: width as u32,
                    height: height as u32,
                    pixels: pixels.into(),
                    geometry: Some(Geometry {
                        nifti_transform: None,
                        frame_of_reference: Some("generated-bricks".into()),
                        position: [0., 0., z as f64 * 1.5],
                        orientation: [1., 0., 0., 0., 1., 0.],
                        spacing: [0.75, 0.5],
                    }),
                    instance: z as i32,
                    frame_index: 0,
                    monochrome1: false,
                    default_window: WindowLevel::SOFT_TISSUE,
                })
            })
            .collect();
        Volume::prepare(frames, || false).unwrap()
    }

    #[test]
    fn the_measured_wide_volume_fits_device_sides_with_bounded_halo_overhead() {
        let layout = BrickLayout::plan([4096, 512, 256], 2048).unwrap();
        assert_eq!(layout.len(), 3);
        for index in 0..layout.len() {
            let brick = layout.brick(index).unwrap();
            assert!(brick.size.iter().all(|side| *side <= 2048), "{brick:?}");
        }
        let dense = 4096u64 * 512 * 256;
        // Two shared halo planes of 512x256 voxels.
        assert_eq!(layout.stored_voxels(), dense + 2 * 512 * 256);
        assert_eq!(BrickLayout::plan([10, 10, 10], 2), Err(BrickError::Limit));
        assert_eq!(BrickLayout::plan([1, 10, 10], 8), Err(BrickError::Limit));
        assert_eq!(
            BrickLayout::plan([8192, 8192, 8192], 3),
            Err(BrickError::Limit)
        );
    }

    #[test]
    fn brick_local_sampling_equals_dense_sampling_across_every_brick_boundary() {
        for missing in [false, true] {
            let volume = volume([10, 7, 6], missing);
            let dimensions = volume.dimensions();
            // Interior 3: bricks meet at voxels 3, 6 and 9 on x.
            let layout = BrickLayout::plan(dimensions, 4).unwrap();
            assert_eq!(layout.len(), 3 * 2 * 2);
            let gpu = MemoryBudget::gpu().used();
            let bricks: Vec<_> = (0..layout.len())
                .map(|index| extract(&volume, layout.brick(index).unwrap(), &|| false).unwrap())
                .collect();
            assert!(MemoryBudget::gpu().used() > gpu);
            let mut compared = 0;
            for z in 0..=20 {
                for y in 0..=24 {
                    for x in 0..=36 {
                        let v = [
                            f64::from(x) * 0.25,
                            f64::from(y) * 0.25,
                            f64::from(z) * 0.25,
                        ];
                        let owner = layout.owner(v).unwrap();
                        let bricked = bricks[owner].0.sample(v, dimensions);
                        let dense = volume.sample_voxel(v);
                        match (dense, bricked) {
                            (Some(a), Some(b)) => {
                                assert!((a - b).abs() <= 1e-4, "{v:?}: {a} vs {b}")
                            }
                            (a, b) => assert_eq!(a, b, "{v:?}"),
                        }
                        compared += usize::from(dense.is_some());
                    }
                }
            }
            assert!(compared > 1000);
            drop(bricks);
            assert_eq!(MemoryBudget::gpu().used(), gpu);
        }
        let volume = volume([10, 7, 6], false);
        let brick = BrickLayout::plan([10, 7, 6], 4).unwrap().brick(0).unwrap();
        assert!(matches!(
            extract(&volume, brick, &|| true),
            Err(BrickError::Cancelled)
        ));
    }

    #[test]
    fn declared_secant_halos_cover_all_six_queries_without_moving_global_sample_positions() {
        for side in [32, 64, 128] {
            for missing in [false, true] {
                let source = volume([side * 2 + 5, side + 7, 9], missing);
                let layout = BrickLayout::with_stencil(
                    source.dimensions(),
                    side,
                    BrickStencil::CentralSecants,
                )
                .unwrap();
                assert_eq!(layout.brick(1).unwrap().size[0], side + 4);
                let rounded = (side as f32).next_down() + 1.;
                assert_eq!(rounded, side as f32 + 1.);
                let page = layout.brick(0).unwrap();
                assert!((rounded as usize + 1) < page.origin[0] + page.size[0]);
                let pages = (0..layout.len())
                    .map(|i| extract(&source, layout.brick(i).unwrap(), &|| false).unwrap())
                    .collect::<Vec<_>>();
                let mut compared = 0;
                for x in [
                    0.,
                    0.25,
                    1.,
                    side as f64 - 0.00001,
                    side as f64,
                    side as f64 + 0.25,
                    side as f64 * 2.,
                    (side * 2 + 4) as f64,
                ] {
                    for y in [0., 0.75, side as f64 - 0.1, (side + 6) as f64] {
                        for z in [0., 0.2, 4.7, 8.] {
                            let p = [x, y, z];
                            let owner = layout.owner(p).unwrap();
                            for axis in 0..3 {
                                for delta in [-1., 0., 1.] {
                                    let mut q = p;
                                    q[axis] = (q[axis] + delta)
                                        .clamp(0., (source.dimensions()[axis] - 1) as f64);
                                    let a = pages[owner].0.sample(q, source.dimensions());
                                    let b = source.sample_voxel(q);
                                    match (a, b) {
                                        (Some(a), Some(b)) => assert!(
                                            (a - b).abs() < 1e-4,
                                            "{side}/{missing}/{p:?}/{q:?}: {a}/{b}"
                                        ),
                                        _ => assert_eq!(a, b),
                                    }
                                    compared += 1;
                                }
                            }
                        }
                    }
                }
                assert!(compared > 1000);
            }
        }
        let source = volume([70, 8, 5], false);
        let tri =
            BrickLayout::with_stencil(source.dimensions(), 32, BrickStencil::Trilinear).unwrap();
        let sec = BrickLayout::with_stencil(source.dimensions(), 32, BrickStencil::CentralSecants)
            .unwrap();
        let p = [32.75, 2.5, 2.];
        assert!(
            extract(&source, tri.brick(0).unwrap(), &|| false)
                .unwrap()
                .0
                .sample(p, source.dimensions())
                .is_none()
        );
        assert!(
            extract(&source, sec.brick(0).unwrap(), &|| false)
                .unwrap()
                .0
                .sample(p, source.dimensions())
                .is_some()
        );
    }

    #[test]
    fn cell_ownership_is_unique_while_halos_overlap_and_the_last_source_node_stays_owned() {
        let dims = [69, 39, 35];
        for stencil in [BrickStencil::Trilinear, BrickStencil::CentralSecants] {
            let layout = BrickLayout::with_stencil(dims, 32, stencil).unwrap();
            let mut cells = vec![0u8; (dims[0] - 1) * (dims[1] - 1) * (dims[2] - 1)];
            for i in 0..layout.len() {
                let [lo, hi] = layout.cells(i).unwrap();
                let b = layout.brick(i).unwrap();
                for a in 0..3 {
                    assert!(b.origin[a] <= lo[a]);
                    assert!(b.origin[a] + b.size[a] > hi[a]);
                }
                for z in lo[2]..hi[2] {
                    for y in lo[1]..hi[1] {
                        for x in lo[0]..hi[0] {
                            let index = (z * (dims[1] - 1) + y) * (dims[0] - 1) + x;
                            cells[index] += 1;
                            assert_eq!(layout.owner([x as f64, y as f64, z as f64]), Some(i));
                        }
                    }
                }
            }
            assert!(cells.iter().all(|n| *n == 1));
            assert_eq!(
                layout.owner(dims.map(|n| (n - 1) as f64)),
                Some(layout.len() - 1)
            );
            assert!(layout.brick(layout.len()).is_none());
            assert!(layout.cells(usize::MAX).is_none());
        }
    }

    #[test]
    fn malformed_brick_shapes_payloads_and_nonfinite_coordinates_are_refused_without_panics() {
        assert_eq!(
            BrickLayout::plan([usize::MAX, 2, 2], usize::MAX),
            Err(BrickError::Limit)
        );
        assert_eq!(
            BrickLayout::with_stencil([8; 3], 16, BrickStencil::CentralSecants),
            Err(BrickError::Limit)
        );
        let source = volume([8; 3], false);
        let layout = BrickLayout::plan([8; 3], 4).unwrap();
        for v in [
            [f64::NAN, 0., 0.],
            [f64::INFINITY, 0., 0.],
            [-0.01, 0., 0.],
            [8., 0., 0.],
        ] {
            assert_eq!(layout.owner(v), None);
        }
        for brick in [
            Brick {
                origin: [usize::MAX, 0, 0],
                size: [2; 3],
            },
            Brick {
                origin: [0; 3],
                size: [0; 3],
            },
            Brick {
                origin: [7; 3],
                size: [2; 3],
            },
        ] {
            assert!(matches!(
                extract(&source, brick, &|| false),
                Err(BrickError::Limit)
            ));
        }
        let b = layout.brick(0).unwrap();
        let mut bad = BrickData {
            brick: b,
            values: vec![],
            valid: None,
        };
        assert_eq!(bad.sample([0.; 3], [8; 3]), None);
        bad.values = vec![1.; b.size.iter().product()];
        bad.valid = Some(vec![]);
        assert_eq!(bad.sample([0.; 3], [8; 3]), None);
        bad.valid = None;
        bad.values[0] = f32::NAN;
        assert_eq!(bad.sample([0.; 3], [8; 3]), None);
        assert_eq!(bad.sample([0.; 3], [0; 3]), None);
        bad.values.fill(1.);
        for dimensions in [[usize::MAX; 3], [1 << 30, 2, 2], [1, 8, 8]] {
            for point in [[0.; 3], [usize::MAX as f64; 3], [f64::MAX; 3]] {
                assert_eq!(bad.sample(point, dimensions), None);
            }
        }
    }
}
