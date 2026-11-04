// SPDX-License-Identifier: GPL-3.0-or-later
//! Generated label edit timings; no image files, persistence or networking.
use orv_core::{
    labels::Labels,
    resources::MemoryBudget,
    study::{Geometry, demo_study},
    volume::Volume,
};
use serde_json::{Value, json};
use std::{collections::VecDeque, error::Error, hint::black_box, sync::Arc, time::Instant};
type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn source() -> Result<Arc<Volume>> {
    let template = demo_study().series.remove(0).frames.remove(0);
    // Source values are irrelevant to the brush workload. Share one generated
    // zero plane; report this explicitly rather than presenting logical bytes as RSS.
    let plane: Arc<[f32]> = vec![0.; 512 * 512].into();
    let frames = (0..128)
        .map(|z| {
            let mut frame = (*template).clone();
            frame.width = 512;
            frame.height = 512;
            frame.pixels = plane.clone();
            frame.geometry = Some(Geometry {
                nifti_transform: None,
                frame_of_reference: Some("generated label benchmark".into()),
                position: [0.05 * f64::from(z), 0., f64::from(z) * 1.2],
                orientation: [1., 0., 0., 0., 1., 0.],
                spacing: [0.7, 0.7],
            });
            frame.instance = z;
            Arc::new(frame)
        })
        .collect();
    Ok(Arc::new(Volume::new(frames)?))
}
fn measure(mut work: impl FnMut(usize) -> Result<()>, samples: usize) -> Result<Value> {
    for i in 0..4 {
        work(i)?;
    }
    let mut times = Vec::new();
    for i in 0..samples {
        let start = Instant::now();
        work(i + 4)?;
        times.push(start.elapsed().as_secs_f64() * 1000.);
    }
    times.sort_by(f64::total_cmp);
    Ok(
        json!({"samples":samples,"median_ms":times[samples/2],"p95_ms":times[(samples*95).div_ceil(100)-1],"max_ms":times[samples-1]}),
    )
}
fn main() -> Result<()> {
    let source = source()?;
    let budget = MemoryBudget::volumes();
    let before = budget.used();
    let empty = measure(
        |_| {
            black_box(Labels::empty(source.clone())?);
            Ok(())
        },
        120,
    )?;
    let mut edits = Vec::new();
    for (name, point, radius) in [("one_tile", [8., 8.], 3), ("four_tiles", [31., 31.], 3)] {
        let mut current = Arc::new(Labels::empty(source.clone())?);
        let mut history = VecDeque::new();
        let timing = measure(
            |i| {
                let mut next = current.try_clone(|| false)?;
                next.paint(32, &[point], radius, 1 + (i % 2) as u8, || false)?;
                history.push_back(std::mem::replace(&mut current, Arc::new(next)));
                while history.len() > 16 {
                    history.pop_front();
                }
                black_box(current.revision());
                Ok(())
            },
            120,
        )?;
        let stats = current.storage_statistics();
        edits.push(json!({"kind":name,"point":point,"radius_pixels":radius,"checkpoint_and_paint":timing,
            "retained_history":history.len(),"tile_side":stats.tile_side,"tiles":stats.tiles,
            "current_allocated_tiles":stats.allocated_tiles,"current_directory_bytes":stats.directory_bytes,
            "current_referenced_tile_bytes":stats.referenced_tile_bytes,"all_label_admission_bytes":budget.used()-before}));
        drop(history);
        drop(current);
        if budget.used() != before {
            return Err("Label edit reservations did not return to baseline".into());
        }
    }
    let mut dense = Labels::empty(source.clone())?;
    let threshold = measure(
        |_| {
            dense = Labels::empty(source.clone())?;
            dense.threshold(None, [-1., 1.], 1, || false)?;
            Ok(())
        },
        3,
    )?;
    let counts = dense.statistics(|| false)?;
    if counts.by_id[1].valid_voxels != 512 * 512 * 128 {
        return Err("Generated threshold did not fill all cells".into());
    }
    let retained = budget.used() - before;
    let checkpoint = measure(
        |_| {
            black_box(dense.try_clone(|| false)?);
            Ok(())
        },
        120,
    )?;
    drop(dense);
    let bytes = vec![1u8; 512 * 512 * 128];
    let byte_copy = measure(
        |_| {
            black_box(black_box(&bytes).to_vec());
            Ok(())
        },
        30,
    )?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema_version":1,"version":env!("CARGO_PKG_VERSION"),"optimized":!cfg!(debug_assertions),
            "source_dimensions":[512,512,128],"source_fixture":"128 source frames share one zero-valued 512x512 f32 plane; this is a label storage workload, not source decode or measured RSS",
            "scope":"Worker CPU construction/checkpoint/paint/threshold; no UI, GPU, image display or file I/O",
            "admission_scope":"Shared label allocation reservations, including referenced snapshots; excludes generated source, comparison byte vector, allocator overhead and process RSS",
            "warmups_per_case":4,"empty_map":empty,"edits":edits,"whole_map_threshold":threshold,
            "full_map_admission_bytes":retained,"shared_full_map_checkpoint":checkpoint,
            "dense_32mib_copy_only":byte_copy,"copy_comparison_scope":"Raw byte copy only, not a reproduction or timing of the previous application",
            "label_admission_restored":budget.used()==before
        }))?
    );
    Ok(())
}
