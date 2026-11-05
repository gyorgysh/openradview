// SPDX-License-Identifier: GPL-3.0-or-later
//! Full-grid verification and bounded COW physical stroke timings.
use super::*;
use orv_core::labels::{BrushAction, BrushShape, PhysicalBrush};
type V3 = [f64; 3];
fn dot(a: V3, b: V3) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| a * b).sum()
}
fn sub(a: V3, b: V3) -> V3 {
    std::array::from_fn(|a0| a[a0] - b[a0])
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn length(a: V3) -> f64 {
    dot(a, a).sqrt()
}
fn inside(p: V3, a: V3, b: V3, r: f64) -> bool {
    let d = sub(b, a);
    let q = sub(p, a);
    let t = dot(q, d);
    let len = length(d);
    if t <= 0. || len == 0. {
        length(q) <= r
    } else if t >= len * len {
        length(sub(p, b)) <= r
    } else {
        length(cross(q, d)) <= r * len
    }
}
fn reference(p: V3, points: &[V3], plane: bool, origin: V3, radius: f64) -> bool {
    let project = |p| {
        if !plane {
            return p;
        }
        let q = sub(p, origin);
        let u = [2f64 / 5f64.sqrt(), -1f64 / 5f64.sqrt(), 0.];
        let v = cross([1. / 3., 2. / 3., 2. / 3.], u);
        [dot(q, u), dot(q, v), 0.]
    };
    if plane && dot(sub(p, origin), [1. / 3., 2. / 3., 2. / 3.]).abs() > 0.55 {
        return false;
    }
    if points.len() == 1 {
        inside(project(p), project(points[0]), project(points[0]), radius)
    } else {
        points
            .windows(2)
            .any(|pair| inside(project(p), project(pair[0]), project(pair[1]), radius))
    }
}
fn distribution(mut ms: Vec<f64>) -> Value {
    ms.sort_by(f64::total_cmp);
    let n = ms.len();
    json!({"samples":n,"median_ms":ms[n/2],"p95_ms":ms[(n*95).div_ceil(100)-1],"max_ms":ms[n-1]})
}
pub(super) fn run() -> Result<()> {
    let source = source()?;
    let budget = MemoryBudget::volumes();
    let before = budget.used();
    let mut cases = Vec::new();
    for (name, voxel_points, plane) in [
        ("sphere_inside_one_tile", vec![[8.; 3]], false),
        ("sphere_at_eight_tiles", vec![[31.5; 3]], false),
        (
            "continuous_swept_sphere",
            vec![
                [20., 20., 31.5],
                [31.5; 3],
                [45., 30., 31.5],
                [65., 50., 40.],
            ],
            false,
        ),
        (
            "oblique_plane_sweep",
            vec![[20., 20., 32.], [65., 50., 32.]],
            true,
        ),
    ] {
        let points = voxel_points
            .iter()
            .map(|p| source.voxel_to_world(*p))
            .collect::<Vec<_>>();
        let origin = source.voxel_to_world([32.; 3]);
        let mut settings = PhysicalBrush {
            label: 7,
            action: BrushAction::Add,
            radius_mm: 2.137,
            shape: if plane {
                BrushShape::Plane {
                    origin_mm: origin,
                    normal: [1., 2., 2.],
                    thickness_mm: 1.1,
                }
            } else {
                BrushShape::Sphere
            },
            clip: None,
            protected_ids: [false; 256],
        };
        let base = Arc::new(Labels::empty(source.clone())?);
        let preview = base.propose_physical_stroke(&settings, &points, || false)?;
        let summary = preview.summary();
        let mut independent_count = 0;
        for z in 0..128 {
            for y in 0..512 {
                for x in 0..512 {
                    // Independent generated source affine, not Volume's transform or
                    // the brush's inverse-derived bounding boxes.
                    let p = [
                        0.7 * x as f64 + 0.05 * z as f64,
                        0.7 * y as f64,
                        1.2 * z as f64,
                    ];
                    let expected = reference(p, &points, plane, origin, settings.radius_mm);
                    if preview.value([x, y, z]) != Some(if expected { 7 } else { 0 }) {
                        return Err(format!(
                            "Physical reference mismatch at {:?} in {name}",
                            [x, y, z]
                        )
                        .into());
                    }
                    independent_count += usize::from(expected);
                }
            }
        }
        if independent_count == 0 || independent_count != summary.changed_voxels {
            return Err("Physical stroke count mismatch".into());
        }
        let filled = preview.apply(&base, &settings, &points, || false)?;
        let allocated = filled.storage_statistics().allocated_tiles;
        let mut erase = settings.clone();
        erase.action = BrushAction::Erase;
        let empty = filled
            .propose_physical_stroke(&erase, &points, || false)?
            .apply(&filled, &erase, &points, || false)?;
        if empty.values().iter().any(|v| *v != 0) || base.values().iter().any(|v| *v != 0) {
            return Err("Physical erase or base ownership mismatch".into());
        }
        drop(empty);
        drop(filled);
        drop(base);
        if budget.used() != before {
            return Err("Physical verification reservations were not restored".into());
        }
        let mut current = Arc::new(Labels::empty(source.clone())?);
        let mut history = VecDeque::new();
        let mut apply_times = Vec::with_capacity(124);
        let mut peak = budget.used();
        let timing = measure(
            |i| {
                settings.action = if i.is_multiple_of(2) {
                    BrushAction::Add
                } else {
                    BrushAction::Erase
                };
                let proposal = current.propose_physical_stroke(&settings, &points, || false)?;
                if proposal.summary().changed_voxels != independent_count {
                    return Err("Timed physical revision changed membership".into());
                }
                peak = peak.max(budget.used());
                let start = Instant::now();
                let next = proposal.apply(&current, &settings, &points, || false)?;
                apply_times.push(start.elapsed().as_secs_f64() * 1000.);
                history.push_back(std::mem::replace(&mut current, next));
                while history.len() > 16 {
                    history.pop_front();
                }
                black_box(current.revision());
                Ok(())
            },
            120,
        )?;
        let applies = distribution(apply_times.into_iter().skip(4).collect());
        cases.push(json!({"case":name,"points_mm":points,"radius_mm":settings.radius_mm,"plane":plane,"plane_origin_mm":plane.then_some(origin),
            "plane_normal":plane.then_some([1.,2.,2.]),"plane_thickness_mm":plane.then_some(1.1),
            "candidate_visits":summary.candidate_visits,"distinct_changed_cells":independent_count,"changed_bounds":summary.changed_bounds,
            "changed_tiles":summary.changed_tiles,"populated_tiles":allocated,"independent_full_grid_verified_cells":512*512*128,
            "worker_proposal_apply_and_history":timing,"apply_only":applies,"retained_history":history.len(),"observed_peak_label_admission_bytes":peak-before}));
        drop(current);
        drop(history);
        if budget.used() != before {
            return Err("Physical stroke history reservations were not restored".into());
        }
        eprintln!("Verified physical {name}");
    }
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"schema_version":3,"version":env!("CARGO_PKG_VERSION"),"optimized":!cfg!(debug_assertions),
        "source_dimensions":[512,512,128],"source_fixture":"128 frames share one zero-valued f32 plane; full affine includes 0.05 mm per-slice shear",
        "scope":"CPU worker proposal, atomic apply and 16-revision history, alternating add/erase; 120 samples after four warmups. Independent full-grid verification and all source construction excluded. No GPU/native input/display, source decoding, RSS or surface visibility authority.",
        "admission_scope":"Label directories/tiles/revisions/proposals (including retained path copies) and live history; source/caller input vectors/timing vectors, allocator and process RSS excluded. Peak observed with a completed proposal, not sampled process peak.",
        "cases":cases,"label_admission_restored":budget.used()==before})
        )?
    );
    Ok(())
}
