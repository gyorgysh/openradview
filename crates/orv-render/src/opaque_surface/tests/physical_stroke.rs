// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{
    label_source::{GpuLabels, UploadKind},
    surface_pick::{GpuSurfacePicker, PickFrame, PickScope, PickState, SurfaceProbeKind},
};
use orv_core::labels::{BrushAction, BrushShape, PhysicalBrush};

fn grid(gpu: &Arc<GpuVolumeSource>, labels: &GpuLabels) -> Vec<u8> {
    let d = gpu.device();
    let q = gpu.queue();
    let texture = labels.texture_view().texture();
    let size = texture.size();
    let stride = size.width.div_ceil(256) * 256;
    let buffer = d.create_buffer(&wgpu::BufferDescriptor {
        label: Some("generated physical stroke grid verification"),
        size: u64::from(stride) * u64::from(size.height) * u64::from(size.depth_or_array_layers),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut e = d.create_command_encoder(&Default::default());
    e.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(size.height),
            },
        },
        size,
    );
    q.submit([e.finish()]);
    let (tx, rx) = mpsc::sync_channel(1);
    buffer.map_async(wgpu::MapMode::Read, .., move |r| {
        let _ = tx.send(r);
    });
    fence(d);
    rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
    let view = buffer.get_mapped_range(..).unwrap();
    let out = view
        .chunks_exact(stride as usize)
        .flat_map(|r| r[..size.width as usize].iter().copied())
        .collect();
    drop(view);
    buffer.unmap();
    out
}
#[test]
fn physical_preview_apply_and_undo_keep_gpu_cells_surface_pixels_and_probe_revisions_coherent() {
    let (d, q) = device(|_| {});
    let cpu = source([67, 9, 5], true, true, 1e6);
    let gpu = GpuVolumeSource::new(&d, &q, cpu.clone(), || false).unwrap();
    let base = Arc::new(Labels::empty(cpu.clone()).unwrap());
    let (old, _) = GpuLabels::upload(gpu.clone(), base.clone(), None, || false).unwrap();
    let old_grid = grid(&gpu, &old);
    assert!(old_grid.iter().all(|v| *v == 0));
    let settings = PhysicalBrush {
        label: 7,
        action: BrushAction::Add,
        radius_mm: 3.,
        shape: BrushShape::Sphere,
        clip: None,
        protected_ids: [false; 256],
    };
    let point = cpu.voxel_to_world([31.5, 4., 2.]);
    let proposal = base
        .propose_physical_stroke(&settings, &[point], || false)
        .unwrap();
    assert_eq!(proposal.summary().changed_tiles, 2);
    let preview = proposal.preview().clone();
    assert!(!proposal.is_current(&base, &settings, &[cpu.voxel_to_world([20., 4., 2.])]));
    let (uploaded, stats) =
        GpuLabels::upload(gpu.clone(), preview.clone(), Some(old.clone()), || false).unwrap();
    assert_eq!(stats.kind, UploadKind::Delta);
    assert_eq!(stats.uploaded_tiles, 2);
    assert_eq!(
        grid(&gpu, &uploaded),
        preview.values().iter().copied().collect::<Vec<_>>()
    );
    assert_eq!(grid(&gpu, &old), old_grid);
    let mut scene = scene(&gpu, MaskOwner::Manual(preview.clone()), 7, [65; 2]);
    scene.material = material(0.2, 0.);
    let mut renderer =
        OpaqueSurfaceRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8Unorm).unwrap();
    let image = renderer
        .render_rows(&scene, 0, 0..65, || false)
        .unwrap()
        .unwrap();
    let original = bytes(&d, &q, image.texture(), 8);
    let frame = PickFrame::new(image, PickScope::new().unwrap());
    let mut current = PickState::from_frame(&frame);
    current.map = MaskOwner::Manual(base.clone());
    let picker = GpuSurfacePicker::new(gpu.clone()).unwrap();
    assert!(picker.probe(&frame, &current, [32; 2], || false).is_err());
    let applied = proposal
        .apply(&base, &settings, &[point], || false)
        .unwrap();
    assert!(Arc::ptr_eq(&applied, &preview));
    let (same, stats) =
        GpuLabels::upload(gpu.clone(), applied.clone(), Some(uploaded.clone()), || {
            false
        })
        .unwrap();
    assert_eq!(stats.kind, UploadKind::Reuse);
    assert!(Arc::ptr_eq(&same, &uploaded));
    current.map = MaskOwner::Manual(applied);
    assert!(current.matches(&frame));
    let mut hits = 0;
    for pixel in [[32, 32], [31, 30], [30, 32], [34, 34], [0, 0]] {
        let probe = picker.probe(&frame, &current, pixel, || false).unwrap();
        assert!(probe.is_current(&current, pixel));
        if probe.kind() == SurfaceProbeKind::Hit {
            assert_eq!(
                probe.hit().unwrap().face,
                reference(scene.surface.request(), pixel).unwrap().face
            );
            hits += 1;
        }
    }
    assert!(hits > 0);
    let (undo, stats) =
        GpuLabels::upload(gpu.clone(), base.clone(), Some(uploaded), || false).unwrap();
    assert_eq!(stats.kind, UploadKind::Delta);
    assert_eq!(stats.uploaded_tiles, 2);
    assert_eq!(grid(&gpu, &undo), old_grid);
    assert_eq!(bytes(&d, &q, frame.image().texture(), 8), original);
    current.map = MaskOwner::Manual(base);
    assert!(!current.matches(&frame));
    assert!(picker.probe(&frame, &current, [32; 2], || false).is_err());
}
