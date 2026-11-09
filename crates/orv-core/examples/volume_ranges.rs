// SPDX-License-Identifier: GPL-3.0-or-later
//! Generated scalar summary preparation, not ray-rendering or native latency.
use orv_core::{
    resources::MemoryBudget,
    study::{Geometry, demo_study},
    volume::Volume,
    volume_material::{MaterialPoint, VolumeMaterial},
    volume_ranges::VolumeRanges,
};
use serde_json::{Value, json};
use std::{hint::black_box, sync::Arc, time::Instant};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn source(kind: &str) -> Result<(Arc<Volume>, [f32; 2], u64)> {
    let template = demo_study().series.remove(0).frames.remove(0);
    let mut frames = Vec::new();
    let mut extrema = [f32::INFINITY, f32::NEG_INFINITY];
    let mut valid_vertices = 0;
    for z in 0..128 {
        let mut frame = (*template).clone();
        let mut pixels = Vec::with_capacity(512 * 512);
        let mut validity = Vec::with_capacity(if kind == "missing" { 512 * 512 } else { 0 });
        for y in 0..512 {
            for x in 0..512 {
                let ellipsoid = ((x as f64 - 220.) / 110.).powi(2)
                    + ((y as f64 - 240.) / 90.).powi(2)
                    + ((z as f64 - 64.) / 44.).powi(2)
                    <= 1.;
                let plate = x == 257 && (200..420).contains(&y);
                let known = kind != "missing" || (z >= 40 && z % 17 != 0);
                let value = if !known {
                    if x % 2 == 0 { -1e20 } else { 1e20 }
                } else if kind == "dense" {
                    (x + 2 * y + 3 * z) as f32
                } else if plate {
                    1500.
                } else if ellipsoid {
                    ((x * 13 + y * 7 + z * 17) % 700 + 100) as f32
                } else {
                    -1000.
                };
                pixels.push(value);
                if kind == "missing" {
                    validity.push(u8::from(known));
                }
                extrema[0] = extrema[0].min(value);
                extrema[1] = extrema[1].max(value);
                valid_vertices += u64::from(known);
            }
        }
        frame.width = 512;
        frame.height = 512;
        frame.pixels = pixels.into();
        frame.validity = (kind == "missing").then(|| validity.into());
        frame.geometry = Some(Geometry {
            position: [z as f64 * 0.05, 0., z as f64 * 1.2],
            orientation: [1., 0., 0., 0., 1., 0.],
            spacing: [0.7, 0.7],
            frame_of_reference: Some("generated range benchmark".into()),
            nifti_transform: None,
        });
        frames.push(Arc::new(frame));
    }
    Ok((Volume::prepare(frames, || false)?, extrema, valid_vertices))
}

fn inspect(grid: &VolumeRanges, extrema: [f32; 2], valid: u64) -> Result<Value> {
    let summary = grid.summaries();
    let lo = summary.iter().map(|s| s.minimum).reduce(f32::min);
    let hi = summary.iter().map(|s| s.maximum).reduce(f32::max);
    if lo != Some(extrema[0])
        || hi != Some(extrema[1])
        || summary
            .iter()
            .map(|s| u64::from(s.valid_vertices))
            .sum::<u64>()
            < valid
    {
        return Err("Generated scalar extrema/coverage mismatch".into());
    }
    let cells: usize = (0..summary.len())
        .filter_map(|i| grid.tile(i))
        .map(|tile| tile.cells.iter().product::<usize>())
        .sum();
    if cells != 511 * 511 * 127 {
        return Err("Interpolation cells were lost or counted twice".into());
    }
    let material = VolumeMaterial::new(&[
        MaterialPoint {
            value: -1000.,
            luminance: 0.,
            extinction_per_mm: 0.,
        },
        MaterialPoint {
            value: -800.,
            luminance: 0.,
            extinction_per_mm: 0.,
        },
        MaterialPoint {
            value: -700.,
            luminance: 0.5,
            extinction_per_mm: 0.02,
        },
        MaterialPoint {
            value: 2000.,
            luminance: 1.,
            extinction_per_mm: 0.02,
        },
    ])?;
    Ok(json!({
        "tiles":summary.len(), "cell_side":grid.side(), "counts":grid.counts(),
        "original_scalar_extrema":extrema,
        "stored_vertices_with_repeated_halos":summary.iter().map(|s|u64::from(s.vertices)).sum::<u64>(),
        "all_valid_tiles":summary.iter().filter(|s|s.all_valid()).count(),
        "all_missing_tiles":summary.iter().filter(|s|s.all_missing()).count(),
        "zero_extinction_vertex_intervals":summary.iter().filter(|s|material.transparent_between(s.minimum,s.maximum)).count(),
        "interval_count_scope":"original vertex extrema only; no rounded-sampler or GPU skip claim",
        "independent_global_extrema_and_cell_coverage_match":true
    }))
}

fn main() -> Result<()> {
    let budget = MemoryBudget::volumes();
    let before = budget.used();
    let mut cases = Vec::new();
    for kind in ["sparse_ellipsoid_and_one_vertex_plate", "missing", "dense"] {
        let (volume, extrema, valid) = source(kind)?;
        let source_charge = budget.used() - before;
        for side in [32, 64, 128] {
            let first = VolumeRanges::build(volume.clone(), side, || false)?;
            let inspection = inspect(&first, extrema, valid)?;
            let summary_charge = budget.used() - before - source_charge;
            drop(first);
            let mut times = Vec::new();
            for _ in 0..7 {
                let start = Instant::now();
                let grid = VolumeRanges::build(volume.clone(), side, || false)?;
                black_box(grid.summaries());
                drop(grid);
                times.push(start.elapsed().as_secs_f64() * 1000.);
                if budget.used() != before + source_charge {
                    return Err("Range preparation reservation was retained".into());
                }
            }
            times.sort_by(f64::total_cmp);
            cases.push(json!({"kind":kind,"inspection":inspection,"cpu_summary_admission_bytes":summary_charge,
                "cpu_source_admission_bytes":source_charge,"samples":7,"warmups":1,
                "median_ms":times[3],"p95_ms":times[6],"max_ms":times[6]}));
        }
        drop(volume);
        if budget.used() != before {
            return Err("Generated source reservation was retained".into());
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema":1,"workload":"generated affine scalar range preparation",
            "dimensions":[512,512,128],"unique_scalar_bytes":512*512*128*4,
            "scope":"worker scan, CPU admission, summary allocation and drop; excludes source construction, GPU upload/rendering, native display and process RSS",
            "reservations_restored":budget.used()==before,"cases":cases
        }))?
    );
    Ok(())
}
