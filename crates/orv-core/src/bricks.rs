// SPDX-License-Identifier: GPL-3.0-or-later
//! Prototype RAM-only brick layout for GPU residency of a dense scalar volume.
//!
//! Measured need: a generated 4096x512x256 f32 volume (2 GiB) is admitted on
//! the CPU but refused by Metal's 2048-voxel 3D texture side. Bricks split the
//! volume into textures within a device side limit. Each stores its interior
//! cells plus a one-voxel halo, so every trilinear cell lies wholly inside the
//! brick that owns its lower corner and brick-local sampling equals dense
//! sampling, including missing samples. The dense CPU volume stays resident:
//! this changes GPU residency only and adds no disk spill or second CPU copy.
use crate::{resources::MemoryBudget, volume::Volume};

/// Bound on brick count, keeping the page table small.
pub const MAX_BRICKS: usize = 4096;

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
}
impl BrickLayout {
    /// `max_side` is the device's largest texture side, at least 3.
    pub fn plan(dimensions: [usize; 3], max_side: usize) -> Result<Self, BrickError> {
        if max_side < 3 || dimensions.iter().any(|d| *d < 2) {
            return Err(BrickError::Limit);
        }
        // A brick owns cells [k*n, (k+1)*n) and stores voxels up to (k+1)*n.
        let interior = [max_side - 1; 3];
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
        })
    }
    pub fn len(&self) -> usize {
        self.counts.iter().product()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn brick(&self, index: usize) -> Option<Brick> {
        if index >= self.len() {
            return None;
        }
        let cell = [
            index % self.counts[0],
            index / self.counts[0] % self.counts[1],
            index / (self.counts[0] * self.counts[1]),
        ];
        let origin: [usize; 3] = std::array::from_fn(|axis| cell[axis] * self.interior[axis]);
        let size = std::array::from_fn(|axis| {
            (origin[axis] + self.interior[axis] + 1).min(self.dimensions[axis]) - origin[axis]
        });
        Some(Brick { origin, size })
    }
    /// Page-table lookup: the brick owning the trilinear cell of voxel `v`.
    pub fn owner(&self, v: [f64; 3]) -> Option<usize> {
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
    let count: usize = brick.size.iter().product();
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
                    if self.valid.as_ref().is_some_and(|valid| valid[index] == 0) {
                        return None;
                    }
                    value += f64::from(self.values[index]) * weight;
                }
            }
        }
        Some(value as f32)
    }
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
                    presentation: None,
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
}
