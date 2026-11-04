// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{
    WindowLevel,
    study::{Frame, Geometry},
};
use std::sync::atomic::{AtomicUsize, Ordering};

fn source(
    dimensions: [usize; 3],
    reversed: bool,
    sample: impl Fn([usize; 3]) -> (f32, bool),
) -> Arc<Volume> {
    let [width, height, depth] = dimensions;
    let sign = if reversed { -1. } else { 1. };
    let mut frames: Vec<_> = (0..depth)
        .map(|z| {
            let mut pixels = Vec::new();
            let mut valid = Vec::new();
            for y in 0..height {
                for x in 0..width {
                    let (value, known) = sample([x, y, z]);
                    pixels.push(value);
                    valid.push(u8::from(known));
                }
            }
            Arc::new(Frame {
                width: width as u32,
                height: height as u32,
                pixels: pixels.into(),
                validity: Some(valid.into()),
                geometry: Some(Geometry {
                    position: [1e8 + sign * z as f64 * 2., -1e8 + z as f64 * 0.25, 9.],
                    orientation: [0., sign, 0., 0., 0., 1.],
                    spacing: [1.5, 0.75],
                    frame_of_reference: Some("generated-ranges".into()),
                    nifti_transform: None,
                }),
                presentation: None,
                calibration: None,
                projection: None,
                sop: None,
                slab: None,
                scalar: None,
                color: None,
                nifti: None,
                rgba: None,
                instance: z as i32,
                frame_index: 0,
                monochrome1: false,
                default_window: WindowLevel::SOFT_TISSUE,
            })
        })
        .collect();
    frames.reverse();
    Arc::new(Volume::new(frames).unwrap())
}

#[test]
fn summaries_match_an_independent_vertex_scan_with_clipped_positive_halos() {
    for reversed in [false, true] {
        let volume = source([20, 19, 18], reversed, |[x, y, z]| {
            (
                ((x * 31 + y * 17 + z * 7) % 107) as f32 - 43.,
                (x + y + z) % 7 != 0,
            )
        });
        let budget = MemoryBudget::new(1024 * 1024);
        let grid = VolumeRanges::build_with_budget(volume.clone(), 8, &budget, &|| false).unwrap();
        assert!(Arc::ptr_eq(grid.source(), &volume));
        assert_eq!(grid.side(), 8);
        assert_eq!(grid.counts(), [3, 3, 3]);
        assert_eq!(grid.summaries().len(), 27);
        let mut visited = 0;
        for index in 0..27 {
            let tile = grid.tile(index).unwrap();
            let mut values = Vec::new();
            let mut valid = 0;
            // Enumerate the original source, independently of tile scan order.
            for (z, frame) in volume.frames().iter().enumerate() {
                for (i, value) in frame.pixels.iter().enumerate() {
                    let point = [i % 20, i / 20, z];
                    if (0..3).all(|a| {
                        point[a] >= tile.origin[a] && point[a] <= tile.origin[a] + tile.cells[a]
                    }) {
                        values.push(*value);
                        valid += u32::from(frame.has_sample(i));
                    }
                }
            }
            assert_eq!(tile.summary.vertices as usize, values.len());
            assert_eq!(tile.summary.valid_vertices, valid);
            assert_eq!(
                tile.summary.minimum,
                values.iter().copied().reduce(f32::min).unwrap()
            );
            assert_eq!(
                tile.summary.maximum,
                values.iter().copied().reduce(f32::max).unwrap()
            );
            assert!(!tile.summary.all_valid());
            assert!(!tile.summary.all_missing());
            visited += values.len();
        }
        assert_eq!(grid.tile(26).unwrap().cells, [3, 2, 1]);
        assert!(visited > volume.dimensions().iter().product::<usize>());
        assert!(grid.tile(27).is_none());
        drop(grid);
        assert_eq!(budget.used(), 0);
    }
}

#[test]
fn one_vertex_features_and_invalid_raw_extrema_survive_all_shared_faces() {
    for axis in 0..3 {
        let volume = source([18; 3], false, |p| {
            if p[axis] == 8 {
                (1e20, false)
            } else if p[axis] == 16 {
                (999., true)
            } else {
                (-5., true)
            }
        });
        let grid = VolumeRanges::build(volume, 8, || false).unwrap();
        for index in 0..grid.summaries().len() {
            let tile = grid.tile(index).unwrap();
            if tile.origin[axis] <= 8 && tile.origin[axis] + tile.cells[axis] >= 8 {
                assert_eq!(tile.summary.maximum, 1e20);
                assert!(!tile.summary.all_valid());
            } else {
                assert_eq!(tile.summary.maximum, 999.);
                assert!(tile.summary.all_valid());
            }
            assert_eq!(tile.summary.minimum, -5.);
        }
    }
    let empty = source([10; 3], false, |_| (123., false));
    let grid = VolumeRanges::build(empty.clone(), 8, || false).unwrap();
    assert!(grid.summaries().iter().all(|s| s.all_missing()));
    assert!(grid.owner([8.5; 3]).is_some());
    assert_eq!(empty.sample_voxel([8.5; 3]), None);
    // One valid node is meaningful, even with no complete valid interior cell.
    let node = source([10; 3], false, |p| (123., p == [8; 3]));
    let grid = VolumeRanges::build(node.clone(), 8, || false).unwrap();
    assert!(grid.summaries().iter().all(|s| s.valid_vertices == 1));
    assert_eq!(node.sample_voxel([8.; 3]), Some(123.));
    assert_eq!(node.sample_voxel([8.01; 3]), None);
}

#[test]
fn ownership_preserves_exact_node_boundaries_and_rejects_outside_coordinates() {
    let grid =
        VolumeRanges::build(source([18, 10, 3], false, |_| (0., true)), 8, || false).unwrap();
    assert_eq!(grid.counts(), [3, 2, 1]);
    for (point, expected) in [
        ([0.; 3], 0),
        ([8.0f64.next_down(), 0., 0.], 0),
        ([8., 0., 0.], 1),
        ([8.0f64.next_up(), 0., 0.], 1),
        ([16., 8., 2.], 5),
        ([17., 9., 2.], 5),
        ([17., 0., 0.], 2),
    ] {
        assert_eq!(grid.owner(point), Some(expected), "{point:?}");
    }
    for point in [
        [-f64::MIN_POSITIVE, 0., 0.],
        [17.0f64.next_up(), 0., 0.],
        [0., 9.0f64.next_up(), 0.],
        [0., 0., 2.0f64.next_up()],
        [f64::NAN, 0., 0.],
        [0., f64::INFINITY, 0.],
    ] {
        assert_eq!(grid.owner(point), None);
    }
}

#[test]
fn vertex_extrema_enclose_ideal_trilinear_values_across_faces_edges_and_corners() {
    let volume = source([18; 3], true, |[x, y, z]| {
        (
            ((x * 29 + y * 17 + z * 13) % 103) as f32 - 51.,
            (x + y + z) % 11 != 0,
        )
    });
    let grid = VolumeRanges::build(volume.clone(), 8, || false).unwrap();
    let positions = [0., 0.25, 7.999, 8., 8.001, 15.999, 16., 16.75, 17.];
    let mut valid = 0;
    for z in positions {
        for y in positions {
            for x in positions {
                let q: [f64; 3] = [x, y, z];
                let summary = grid.tile(grid.owner(q).unwrap()).unwrap().summary;
                let lower = q.map(|p| p.floor() as usize);
                let mut value = 0.;
                for corner in 0..8 {
                    let high = [corner & 1 != 0, corner & 2 != 0, corner & 4 != 0];
                    let position: [usize; 3] =
                        std::array::from_fn(|a| (lower[a] + usize::from(high[a])).min(17));
                    let weight: f64 = (0..3)
                        .map(|a| {
                            if high[a] {
                                q[a].fract()
                            } else {
                                1. - q[a].fract()
                            }
                        })
                        .product();
                    value += f64::from(
                        volume.frames()[position[2]].pixels[position[1] * 18 + position[0]],
                    ) * weight;
                }
                assert!(value >= f64::from(summary.minimum) && value <= f64::from(summary.maximum));
                if let Some(actual) = volume.sample_voxel(q) {
                    assert!(!summary.all_missing());
                    assert!((f64::from(actual) - value).abs() < 1e-5);
                    valid += 1;
                }
            }
        }
    }
    assert!(valid > 200);
}

#[test]
fn admission_cancellation_and_source_lifetimes_are_bounded() {
    let volume = source([20; 3], false, |_| (7., true));
    let weak = Arc::downgrade(&volume);
    let zero = MemoryBudget::new(0);
    assert!(matches!(
        VolumeRanges::build_with_budget(volume.clone(), 8, &zero, &|| false),
        Err(RangeError::Memory)
    ));
    assert!(matches!(
        VolumeRanges::build(volume.clone(), 8, || true),
        Err(RangeError::Cancelled)
    ));
    let budget = MemoryBudget::new(4096);
    let calls = AtomicUsize::new(0);
    assert!(matches!(
        VolumeRanges::build_with_budget(volume.clone(), 8, &budget, &|| calls
            .fetch_add(1, Ordering::Relaxed)
            >= 20),
        Err(RangeError::Cancelled)
    ));
    assert_eq!(budget.used(), 0);
    let grid = VolumeRanges::build_with_budget(volume.clone(), 8, &budget, &|| false).unwrap();
    let bytes = 27 * std::mem::size_of::<ScalarSummary>()
        + std::mem::size_of::<VolumeRanges>()
        + 2 * std::mem::size_of::<usize>();
    assert_eq!(budget.used(), bytes as u64);
    let second = source([20; 3], false, |_| (7., true));
    assert!(!Arc::ptr_eq(grid.source(), &second));
    drop(volume);
    assert!(weak.upgrade().is_some());
    budget.set_limit(0);
    assert!(matches!(
        VolumeRanges::build_with_budget(second, 8, &budget, &|| false),
        Err(RangeError::Memory)
    ));
    drop(grid);
    assert!(weak.upgrade().is_none());
    assert_eq!(budget.used(), 0);
}

#[test]
fn layouts_refuse_unbounded_metadata_or_invalid_sides_before_scanning() {
    assert_eq!(layout([4097, 4097, 4097], 64), Ok([64; 3]));
    assert_eq!(layout([4161, 4161, 4161], 64), Err(RangeError::Layout));
    assert_eq!(layout([4097, 4097, 4097], 128), Ok([32; 3]));
    assert_eq!(layout([usize::MAX; 3], 8), Err(RangeError::Layout));
    assert_eq!(layout([4097, 4097, 4097], 8), Err(RangeError::Layout));
    assert_eq!(layout([1, 2, 2], 8), Err(RangeError::Layout));
    assert_eq!(layout([2; 3], 8), Ok([1; 3]));
    for side in [0, 1, 7, 9, 63, 129, usize::MAX] {
        assert_eq!(layout([10; 3], side), Err(RangeError::Layout));
    }
    for side in [8, 16, 32, 64, 128] {
        let grid =
            VolumeRanges::build(source([2; 3], false, |_| (0., true)), side, || false).unwrap();
        assert_eq!(grid.summaries().len(), 1);
        assert_eq!(grid.tile(0).unwrap().cells, [1; 3]);
        assert_eq!(grid.tile(0).unwrap().summary.vertices, 8);
    }
    let scalar = source([2; 3], false, |_| (0., true));
    let frames = scalar
        .frames()
        .iter()
        .map(|frame| {
            let mut color = (**frame).clone();
            color.pixels = Arc::from([]);
            color.validity = None;
            color.rgba = Some(vec![255; 16].into());
            Arc::new(color)
        })
        .collect();
    let color = Arc::new(Volume::new(frames).unwrap());
    assert!(matches!(
        VolumeRanges::build(color, 8, || false),
        Err(RangeError::Color)
    ));
}
