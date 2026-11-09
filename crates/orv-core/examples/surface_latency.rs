// SPDX-License-Identifier: GPL-3.0-or-later
//! Generated worker CPU boundaries and incremental updates; no files or network.
use orv_core::{
    labels::Labels,
    mask::MaskOwner,
    resources::MemoryBudget,
    study::{Geometry, demo_study},
    surface::{CellSurface, SurfaceLimits, SurfaceUpdate},
    volume::Volume,
};
use serde_json::{Value, json};
use std::{hint::black_box, sync::Arc, time::Instant};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn source() -> Result<Arc<Volume>> {
    let template = demo_study().series.remove(0).frames.remove(0);
    let plane: Arc<[f32]> = vec![0.; 512 * 512].into();
    let frames = (0..128)
        .map(|z| {
            let mut f = (*template).clone();
            f.width = 512;
            f.height = 512;
            f.pixels = plane.clone();
            f.geometry = Some(Geometry {
                position: [f64::from(z) * 0.05, 0., f64::from(z) * 1.2],
                orientation: [1., 0., 0., 0., 1., 0.],
                spacing: [0.7, 0.7],
                frame_of_reference: Some("generated surface benchmark".into()),
                nifti_transform: None,
            });
            Arc::new(f)
        })
        .collect();
    Ok(Arc::new(Volume::new(frames)?))
}
fn measure(
    mut run: impl FnMut(usize) -> Result<()>,
    samples: usize,
    warmups: usize,
) -> Result<Value> {
    for i in 0..warmups {
        run(i)?;
    }
    let mut times = Vec::new();
    for i in 0..samples {
        let start = Instant::now();
        run(i + warmups)?;
        times.push(start.elapsed().as_secs_f64() * 1000.);
    }
    times.sort_by(f64::total_cmp);
    Ok(
        json!({"samples":samples,"warmups":warmups,"median_ms":times[samples/2],"p95_ms":times[(samples*95).div_ceil(100)-1],"max_ms":times[samples-1]}),
    )
}
fn validate(surface: &CellSurface) -> Result<usize> {
    let view = surface.owner().view(surface.key().number)?;
    let dimensions = surface.source().dimensions();
    let mut count = 0;
    for chunk in surface.chunks() {
        for face in chunk.faces() {
            let cell = face.cell().map(|p| p as usize);
            if !view.sample(cell).is_some_and(|s| s.eligible()) {
                return Err("Ineligible generated surface face".into());
            }
            let a = face.side().axis();
            let mut neighbor = cell;
            if let Some(n) = cell[a]
                .checked_add_signed(if face.side().positive() { 1 } else { -1 })
                .filter(|n| *n < dimensions[a])
            {
                neighbor[a] = n;
                let sample = view.sample(neighbor).ok_or("Missing generated neighbor")?;
                if !sample.known || !sample.source_valid || sample.present {
                    return Err("Unsupported generated surface boundary".into());
                }
            }
            if count % 4096 == 0 {
                black_box(surface.physical_face(face)?);
            }
            count += 1;
        }
    }
    if count != surface.face_count() {
        return Err("Generated surface count mismatch".into());
    }
    Ok(count)
}
fn main() -> Result<()> {
    let source = source()?;
    let budget = MemoryBudget::volumes();
    let baseline = budget.used();
    let mut cases = Vec::new();
    for (kind, slice, expected) in [
        ("empty", None, 0),
        ("filled", None, 786432),
        ("one_cell_plane", Some(64), 526336),
    ] {
        let mut map = Labels::empty(source.clone())?;
        if kind != "empty" {
            map.threshold(slice, [-1., 1.], 1, || false)?;
        }
        let map = Arc::new(map);
        let mut latest = None;
        let mut update = SurfaceUpdate::default();
        let initial = measure(
            |_| {
                // Fresh output: no implicit reuse of the prior timed surface.
                latest = None;
                let (surface, stats) = CellSurface::build(
                    MaskOwner::Manual(map.clone()),
                    1,
                    None,
                    SurfaceLimits::default(),
                    || false,
                )?;
                update = stats;
                latest = Some(surface);
                Ok(())
            },
            3,
            1,
        )?;
        let first = latest.take().ok_or("Generated surface missing")?;
        if first.face_count() != expected {
            return Err("Analytic generated boundary count mismatch".into());
        }
        let verified = validate(&first)?;
        let reuse = measure(
            |_| {
                let (same, update) = CellSurface::build(
                    MaskOwner::Manual(map.clone()),
                    1,
                    Some(&first),
                    SurfaceLimits::default(),
                    || false,
                )?;
                if !Arc::ptr_eq(&same, &first)
                    || update.allocated_bytes != 0
                    || update.counted_cells != 0
                {
                    return Err("Unchanged surface was rebuilt".into());
                }
                Ok(())
            },
            120,
            4,
        )?;
        let initial_stats = update;
        let referenced_bytes = first.referenced_bytes();
        let first_charge = budget.used() - baseline;
        let mut edits = Vec::new();
        for (name, point) in [("one_tile", [8., 8.]), ("four_tiles", [31., 31.])] {
            let mut current_map = map.clone();
            let mut current = first.clone();
            let mut last = SurfaceUpdate::default();
            let timing = measure(
                |i| {
                    let mut next = current_map.try_clone(|| false)?;
                    let number = if kind == "empty" {
                        if i % 2 == 0 { 1 } else { 0 }
                    } else if i % 2 == 0 {
                        0
                    } else {
                        1
                    };
                    next.paint(64, &[point], 2, number, || false)?;
                    let next = Arc::new(next);
                    let (surface, stats) = CellSurface::build(
                        MaskOwner::Manual(next.clone()),
                        1,
                        Some(&current),
                        SurfaceLimits::default(),
                        || false,
                    )?;
                    current_map = next;
                    current = surface;
                    last = stats;
                    black_box(&current);
                    Ok(())
                },
                30,
                4,
            )?;
            let verified = validate(&current)?;
            let (fresh, _) = CellSurface::build(
                MaskOwner::Manual(current_map),
                1,
                None,
                SurfaceLimits::default(),
                || false,
            )?;
            let comparison_bytes = ((current.face_count() + fresh.face_count())
                * std::mem::size_of::<orv_core::surface::CellFace>())
                as u64;
            let _comparison = budget
                .reserve(comparison_bytes)
                .ok_or("Generated comparison staging admission refused")?;
            let mut actual = Vec::new();
            actual.try_reserve_exact(current.face_count())?;
            actual.extend(current.chunks().flat_map(|c| c.faces().iter().copied()));
            let mut expected = Vec::new();
            expected.try_reserve_exact(fresh.face_count())?;
            expected.extend(fresh.chunks().flat_map(|c| c.faces().iter().copied()));
            actual.sort();
            expected.sort();
            if actual != expected {
                return Err("Incremental surface differs from fresh extraction".into());
            }
            edits.push(json!({"case":name,"point":point,"radius_pixels":2,"checkpoint_brush_and_surface":timing,
                "rebuilt_chunks":last.rebuilt_chunks,"reused_chunks":last.reused_chunks,"counted_cells":last.counted_cells,
                "emitted_cells":last.emitted_cells,"new_storage_admission_bytes":last.allocated_bytes,
                "verified_faces":verified,"fresh_reference_equal":true}));
        }
        cases.push(json!({"case":kind,"fresh_build":initial,"fresh_counted_cells":initial_stats.counted_cells,
            "fresh_emitted_cells":initial_stats.emitted_cells,"face_count":first.face_count(),"verified_faces":verified,
            "referenced_surface_bytes":referenced_bytes,"mask_and_surface_admission_bytes":first_charge,
            "unchanged_owner_reuse":reuse,"edits":edits}));
        drop(first);
        drop(map);
        if budget.used() != baseline {
            return Err("Surface reservations did not return to baseline".into());
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"schema":1,"dimensions":[512,512,128],"chunk_side":32,
        "source_logical_bytes":512*512*128*4,"shared_source_plane_bytes":512*512*4,"cases":cases,
        "scope":"Worker CPU exact cell-boundary descriptors and edited chunk/face-neighbor updates. Fresh build includes counting, halo reads, allocation and emission. Edits include checkpoint and paint, retaining the original map/surface and latest revision. Untimed verification checks every face against membership and compares each edited output to fresh extraction. Source frames share one constant plane; logical bytes and admission are not RSS. No file decoding, GPU, expanded mesh buffers, smoothing, native input or display timing. Reuse timings are functional checks near clock resolution."})
        )?
    );
    Ok(())
}
