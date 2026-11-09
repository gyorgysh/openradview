// SPDX-License-Identifier: GPL-3.0-or-later
//! Generated complete visible-layer correction work; no native input or presentation.
#[path = "fixtures/optical.rs"]
mod optical;
use orv_core::{
    ViewState,
    labels::{BrushAction, BrushShape, Labels, PhysicalBrush},
    mask::{MaskOwner, MaskView},
    resources::MemoryBudget,
    surface::{CellSurface, InterfaceRay, SurfaceLimits},
    volume_material::{MaterialPoint, VolumeMaterial},
    volume_view::{Projection, Quality, VolumeView},
};
use orv_render::{
    material_volume::Lighting,
    section_volume::pick::{SectionFaceErase, SectionPickFrame, SectionPickState},
    section_volume::{SectionRenderer, SectionScene, SectionSurface},
    surface_pick::{PickScope, SurfaceProbeKind},
    surface_projection::{GpuSurfaceProjector, SurfaceProjection},
    surface_source::GpuCellSurface,
    volume_source::GpuVolumeSource,
};
use serde_json::{Value, json};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn fence(device: &wgpu::Device) -> Result<()> {
    device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: Some(Duration::from_secs(60)),
    })?;
    Ok(())
}
fn timing(mut values: Vec<f64>, warmups: usize) -> Value {
    values.sort_by(f64::total_cmp);
    let n = values.len();
    json!({"samples":n,"warmups":warmups,"median_ms":values[n/2],
        "p95_ms":values[(n*95).div_ceil(100)-1],"max_ms":values[n-1]})
}
fn verify(
    renderer: &mut SectionRenderer,
    frame: &SectionPickFrame,
    current: &SectionPickState,
    proposal: &SectionFaceErase,
    region: [[u32; 2]; 2],
) -> Result<Value> {
    let base = proposal.proposal().base();
    let source = base.source();
    let [w, h, d] = source.dimensions();
    let words = (w * h * d).div_ceil(64);
    let _memory = MemoryBudget::volumes()
        .reserve((words * 8) as u64)
        .ok_or("Stamp verification bitset admission refused")?;
    let mut expected = Vec::new();
    expected.try_reserve_exact(words)?;
    expected.resize(words, 0u64);
    let camera = current.scene.volume.camera.frame(
        &current.scene.navigation,
        f64::from(current.scene.size[0]) / f64::from(current.scene.size[1]),
    )?;
    let settings = proposal.proposal().settings();
    let centre = proposal.proposal().points_mm()[0];
    let mut checked_rays = 0;
    let mut distinct = 0;
    let mut counts = [0usize; 4];
    for y in (0..region[1][1]).step_by(16) {
        for x in (0..region[1][0]).step_by(256) {
            let batch = renderer.probe_region(
                frame,
                current,
                [
                    [region[0][0] + x, region[0][1] + y],
                    [(region[1][0] - x).min(256), (region[1][1] - y).min(16)],
                ],
                || false,
            )?;
            for sample in batch.samples() {
                counts[match sample.kind() {
                    SurfaceProbeKind::Hit => 0,
                    SurfaceProbeKind::Empty => 1,
                    SurfaceProbeKind::Obscured => 2,
                    SurfaceProbeKind::Ambiguous => 3,
                }] += 1;
                let Some(hit) = sample.hit() else { continue };
                let (origin, direction) = camera.ray(std::array::from_fn(|a| {
                    (f64::from(sample.pixel()[a]) + 0.5) / f64::from(current.scene.size[a])
                }));
                let independent = InterfaceRay::new(
                    MaskView::manual(base, 1)?,
                    origin,
                    direction,
                    [-1e30, 1e30],
                    None,
                )?
                .first_hit(|| false)?
                .ok_or("Checked stamp face has no f64 reference")?;
                if independent.face != hit.face
                    || independent.key != hit.key
                    || independent.revision != hit.revision
                {
                    return Err("Stamp face and independent source membership differ".into());
                }
                checked_rays += 1;
                let cell = hit.face.cell().map(|v| v as usize);
                let p = source.voxel_to_world(cell.map(|v| v as f64));
                let delta: [f64; 3] = std::array::from_fn(|a| p[a] - centre[a]);
                if delta[0].hypot(delta[1]).hypot(delta[2]) <= settings.radius_mm {
                    let index = (cell[2] * h + cell[1]) * w + cell[0];
                    let bit = 1u64 << (index % 64);
                    if expected[index / 64] & bit == 0 {
                        distinct += 1;
                    }
                    expected[index / 64] |= bit;
                }
            }
        }
    }
    for (index, (before, after)) in base
        .values()
        .iter()
        .zip(proposal.proposal().preview().values().iter())
        .enumerate()
    {
        let value = if expected[index / 64] & (1u64 << (index % 64)) != 0 {
            0
        } else {
            *before
        };
        if *after != value {
            return Err("Prepared stamp differs from the complete source-grid reference".into());
        }
    }
    let visibility = proposal.visibility();
    if distinct != proposal.summary().changed_voxels
        || distinct == 0
        || counts
            != [
                visibility.hit,
                visibility.empty,
                visibility.obscured,
                visibility.ambiguous,
            ]
    {
        return Err("Stamp candidate or changed-cell summary differs from its reference".into());
    }
    Ok(
        json!({"original_roi_pixels":region[1][0]*region[1][1],"repeated_outcomes_hit_empty_obscured_ambiguous":counts,
        "independent_f64_hit_rays":checked_rays,"exact_verified_source_cells":w*h*d,
        "changed_cells":distinct,"crop_coverage_reference":"complete-raster affine/clipping fixtures are checked by GPU unit tests; this workload verifies every repeated ROI Hit against f64 membership and every source cell against the independent physical-sphere selection"}),
    )
}

fn measure(
    renderer: &mut SectionRenderer,
    frame: &SectionPickFrame,
    current: &SectionPickState,
    pixel: [u32; 2],
    settings: &PhysicalBrush,
) -> Result<Value> {
    let device = frame.image().source().device();
    let start = Instant::now();
    let (reference, region) =
        renderer.propose_erase_stamp(frame, current, pixel, settings, || false)?;
    fence(device)?;
    let first_ms = start.elapsed().as_secs_f64() * 1000.;
    let verification = verify(renderer, frame, current, &reference, region)?;
    let (samples, warmups) = if current.scene.size[0] == 512 {
        (60, 4)
    } else {
        (12, 3)
    };
    let mut previous = None;
    let mut times = Vec::new();
    for index in 0..samples + warmups {
        let start = Instant::now();
        let (next, bounds) =
            renderer.propose_erase_stamp(frame, current, pixel, settings, || false)?;
        drop(previous.replace(next));
        fence(device)?;
        let elapsed = start.elapsed().as_secs_f64() * 1000.;
        let next = previous.as_ref().ok_or("Stamp result unavailable")?;
        if bounds != region
            || next.summary() != reference.summary()
            || next.visibility() != reference.visibility()
            || next.proposal().faces() != reference.proposal().faces()
            || !next.is_current(current, settings, reference.proposal().points_mm())
        {
            return Err("Repeated stamp changed its original checked selection".into());
        }
        if index >= warmups {
            times.push(elapsed);
        }
    }
    let prepared = previous.take().ok_or("Prepared stamp unavailable")?;
    let start = Instant::now();
    let result = prepared.apply(current, settings, reference.proposal().points_mm(), || {
        false
    })?;
    let apply_ms = start.elapsed().as_secs_f64() * 1000.;
    if !result
        .values()
        .iter()
        .eq(reference.proposal().preview().values().iter())
    {
        return Err("Prepared Apply changed the verified preview".into());
    }
    drop(result);
    Ok(
        json!({"radius_mm":settings.radius_mm,"pixel":pixel,"region":region,
        "first_stamp_ms":first_ms,"worker":timing(times,warmups),"prepared_apply_once_ms":apply_ms,
        "verification":verification,"summary":{"changed_voxels":reference.summary().changed_voxels,
        "changed_tiles":reference.summary().changed_tiles,"candidate_faces":reference.summary().candidate_faces,
        "candidate_tests":reference.summary().candidate_tests},
        "scope":"original image anchor probe, conservative full-raster crop, all bounded GPU/readback/color/f64 proofs, private COW proposal, previous preview/transient batch drop and retirement fence; reference and previous preview retained; source/geometry/image preparation, repeat/full-grid verification and native input/presentation excluded",
        "prepared_apply_scope":"one consuming current-scene/base/path/settings gate and prepared Arc result only; no voxel scan; single sample includes timer overhead/quantization, excludes full-grid result comparison"}),
    )
}

fn main() -> Result<()> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        flags: orv_render::instance_flags(),
        backends: orv_render::configured_backends()?,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))?;
    let (device, queue) =
        pollster::block_on(adapter.request_device(&orv_render::device_descriptor(&adapter)))?;
    let before = [MemoryBudget::volumes().used(), MemoryBudget::gpu().used()];
    let mut cases = Vec::new();
    for kind in ["dense", "missing"] {
        let cpu = optical::source(kind)?;
        let start = Instant::now();
        let gpu = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false)?;
        let upload_ms = start.elapsed().as_secs_f64() * 1000.;
        let mut brush = PhysicalBrush {
            label: 1,
            action: BrushAction::Add,
            radius_mm: 30.,
            shape: BrushShape::Sphere,
            clip: None,
            protected_ids: [false; 256],
        };
        let start = Instant::now();
        let empty = Arc::new(Labels::empty(cpu.clone())?);
        let stroke = empty.propose_physical_stroke(&brush, &[cpu.center()], || false)?;
        let base = stroke.preview().clone();
        drop(stroke);
        drop(empty);
        let geometry = CellSurface::build(
            MaskOwner::Manual(base),
            1,
            None,
            SurfaceLimits::default(),
            || false,
        )?
        .0;
        let surface = GpuCellSurface::upload(gpu.clone(), geometry, None, || false)?.0;
        let geometry_ms = start.elapsed().as_secs_f64() * 1000.;
        let start = Instant::now();
        let projector = GpuSurfaceProjector::new(gpu.clone())?;
        let mut renderer = SectionRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8UnormSrgb)?;
        let pipeline_ms = start.elapsed().as_secs_f64() * 1000.;
        let mut scene = SectionScene {
            surface: None,
            planes: [None, None, None],
            navigation: ViewState::default(),
            volume: VolumeView {
                projection: Projection::Composite,
                quality: Quality::Refined,
                ..VolumeView::fit(&cpu)
            },
            material: Arc::new(VolumeMaterial::new(&[-1e20, 1e20].map(|value| {
                MaterialPoint {
                    value,
                    luminance: 0.3,
                    extinction_per_mm: 0.,
                }
            }))?),
            lighting: Lighting::UNLIT,
            size: [512, 512],
        };
        scene.volume.camera.yaw_degrees = 23.;
        scene.volume.camera.pitch_degrees = -17.;
        scene.volume.camera.roll_degrees = 11.;
        brush.action = BrushAction::Erase;
        let mut sizes = Vec::new();
        for side in [512, 2048] {
            scene.size = [side, side];
            let start = Instant::now();
            scene.surface = Some(SectionSurface {
                image: projector.render(
                    SurfaceProjection {
                        surface: surface.clone(),
                        camera: scene.volume.camera.clone(),
                        display: scene.navigation.clone(),
                        clip: None,
                        size: scene.size,
                    },
                    || false,
                )?,
                luminance: 0.8,
                lighting: Lighting::HEADLIGHT,
            });
            let image = renderer
                .render_rows(&scene, 0, 0..side, || false)?
                .ok_or("Reference stamp image did not complete")?;
            fence(&device)?;
            let image_ms = start.elapsed().as_secs_f64() * 1000.;
            let frame = SectionPickFrame::new(image, PickScope::new()?);
            let current = SectionPickState::from_frame(&frame)?;
            let mut radii = Vec::new();
            for radius in [2., 5., 10.] {
                brush.radius_mm = radius;
                radii.push(measure(
                    &mut renderer,
                    &frame,
                    &current,
                    [side / 2, side / 2],
                    &brush,
                )?);
            }
            sizes.push(
                json!({"size":[side,side],"reference_image_preparation_ms":image_ms,"radii":radii}),
            );
        }
        cases.push(json!({"source":kind,"dimensions":[512,512,128],"unique_scalar_planes":true,"source_upload_ms":upload_ms,"sphere_geometry_ms":geometry_ms,"pipeline_ms":pipeline_ms,"surface_faces":surface.surface().face_count(),"sizes":sizes}));
    }
    fence(&device)?;
    let after = [MemoryBudget::volumes().used(), MemoryBudget::gpu().used()];
    if before != after {
        return Err("Stamp benchmark did not release all reservations".into());
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"schema":1,"adapter":adapter.get_info().name,
        "cases":cases,"fixture":"one opaque 30mm generated sphere mask; no source planes; zero source extinction; fixed oblique camera; each proposal starts from the same original base, never drills another layer",
        "admission_before":before,"admission_after":after,"admission_is_rss_or_driver_memory":false}))?
    );
    Ok(())
}
