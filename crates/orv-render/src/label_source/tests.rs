// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use orv_core::{
    study::{Geometry, demo_study},
    volume::Volume,
};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};
fn device() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        flags: crate::instance_flags(),
        backends: crate::configured_backends().unwrap(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .expect("Actual GPU required");
    pollster::block_on(adapter.request_device(&crate::device_descriptor(&adapter))).unwrap()
}
fn source() -> Arc<Volume> {
    let template = demo_study().series.remove(0).frames.remove(0);
    Arc::new(
        Volume::new(
            (0..34)
                .map(|z| {
                    let mut f = (*template).clone();
                    f.width = 67;
                    f.height = 35;
                    f.pixels = vec![10.; 67 * 35].into();
                    let mut valid = vec![1; 67 * 35];
                    if z == 0 {
                        valid[0] = 0;
                    }
                    f.validity = Some(valid.into());
                    f.geometry = Some(Geometry {
                        position: [10. + 0.25 * f64::from(z), 20., 30. + 2. * f64::from(z)],
                        orientation: [1., 0., 0., 0., 1., 0.],
                        spacing: [1.5, 0.75],
                        frame_of_reference: Some("generated label GPU grid".into()),
                        nifti_transform: None,
                    });
                    Arc::new(f)
                })
                .collect(),
        )
        .unwrap(),
    )
}
fn read(upload: &GpuLabels) -> Vec<u8> {
    let device = upload.source.device();
    let queue = upload.source.queue();
    let [w, h, d] = upload.labels.source().dimensions();
    let stride = (w as u32).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("generated categorical readback"),
        size: u64::from(stride) * h as u64 * d as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &upload.texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(h as u32),
            },
        },
        wgpu::Extent3d {
            width: w as u32,
            height: h as u32,
            depth_or_array_layers: d as u32,
        },
    );
    queue.submit([encoder.finish()]);
    let (tx, rx) = mpsc::channel();
    buffer.map_async(wgpu::MapMode::Read, .., move |r| {
        tx.send(r).unwrap();
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(30)),
        })
        .unwrap();
    rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
    let bytes = buffer.get_mapped_range(..).unwrap();
    let mut result = Vec::new();
    for row in bytes.chunks_exact(stride as usize) {
        result.extend_from_slice(&row[..w]);
    }
    drop(bytes);
    buffer.unmap();
    result
}
#[test]
fn actual_gpu_label_revisions_upload_only_dirty_tiles_and_preserve_every_old_texture() {
    let (device, queue) = device();
    let volume = source();
    let gpu_source = GpuVolumeSource::new(&device, &queue, volume.clone(), || false).unwrap();
    let blank = Arc::new(Labels::empty(volume).unwrap());
    let (base, stats) =
        GpuLabels::upload(gpu_source.clone(), blank.clone(), None, || false).unwrap();
    assert_eq!(
        stats,
        UploadStatistics {
            kind: UploadKind::Full,
            uploaded_tiles: 12,
            uploaded_bytes: 67 * 35 * 34,
            copied_bytes: 0
        }
    );
    assert!(read(&base).iter().all(|v| *v == 0));
    let mut edge = blank.try_clone(|| false).unwrap();
    edge.paint(33, &[[65., 33.]], 1, 9, || false).unwrap();
    let edge = Arc::new(edge);
    let (first, stats) =
        GpuLabels::upload(gpu_source.clone(), edge.clone(), Some(base.clone()), || {
            false
        })
        .unwrap();
    assert_eq!(
        stats,
        UploadStatistics {
            kind: UploadKind::Delta,
            uploaded_tiles: 1,
            uploaded_bytes: 18,
            copied_bytes: 67 * 35 * 34
        }
    );
    let values = read(&first);
    assert_eq!(values[(33 * 35 + 33) * 67 + 65], 9);
    assert_eq!(values.iter().filter(|v| **v == 9).count(), 5);
    assert!(values.iter().eq(edge.values().iter()));
    assert!(read(&base).iter().all(|v| *v == 0));
    let mut four = edge.try_clone(|| false).unwrap();
    four.paint(1, &[[31., 31.]], 3, 4, || false).unwrap();
    let four = Arc::new(four);
    let (second, stats) = GpuLabels::upload(
        gpu_source.clone(),
        four.clone(),
        Some(first.clone()),
        || false,
    )
    .unwrap();
    assert_eq!(stats.uploaded_tiles, 4);
    assert_eq!(stats.uploaded_bytes, 71680);
    assert!(read(&second).iter().eq(four.values().iter()));
    assert!(read(&first).iter().eq(edge.values().iter()));
    let (undo, stats) = GpuLabels::upload(
        gpu_source.clone(),
        blank.clone(),
        Some(second.clone()),
        || false,
    )
    .unwrap();
    assert_eq!(stats.kind, UploadKind::Delta);
    assert_eq!(stats.uploaded_tiles, 5);
    assert!(read(&undo).iter().all(|v| *v == 0));
    assert!(read(&second).iter().eq(four.values().iter()));
    let (reused, stats) = GpuLabels::upload(
        gpu_source.clone(),
        Arc::new(blank.try_clone(|| false).unwrap()),
        Some(undo.clone()),
        || false,
    )
    .unwrap();
    assert!(Arc::ptr_eq(&reused, &undo));
    assert_eq!(
        stats,
        UploadStatistics {
            kind: UploadKind::Reuse,
            uploaded_tiles: 0,
            uploaded_bytes: 0,
            copied_bytes: 0
        }
    );
    let replacement = Arc::new(Labels::empty(blank.source_arc()).unwrap());
    let (replaced, stats) =
        GpuLabels::upload(gpu_source, replacement, Some(reused), || false).unwrap();
    assert_eq!(stats.kind, UploadKind::Full);
    assert_eq!(stats.copied_bytes, 0);
    assert!(read(&replaced).iter().all(|v| *v == 0));
}
#[test]
fn wrong_sources_and_cancelled_partial_uploads_leave_previous_cells_unchanged() {
    let (device, queue) = device();
    let volume = source();
    let gpu_source = GpuVolumeSource::new(&device, &queue, volume.clone(), || false).unwrap();
    let blank = Arc::new(Labels::empty(volume.clone()).unwrap());
    let (base, _) = GpuLabels::upload(gpu_source.clone(), blank.clone(), None, || false).unwrap();
    let other = Arc::new(Labels::empty(source()).unwrap());
    assert!(GpuLabels::upload(gpu_source.clone(), other, None, || false).is_err());
    let mut changed = blank.try_clone(|| false).unwrap();
    changed.paint(0, &[[0., 0.]], 1, 8, || false).unwrap();
    let changed = Arc::new(changed);
    assert_eq!(
        changed.value([0, 0, 0]),
        Some(8),
        "Stored labels on missing source samples must survive upload"
    );
    let (uploaded, _) = GpuLabels::upload(
        gpu_source.clone(),
        changed.clone(),
        Some(base.clone()),
        || false,
    )
    .unwrap();
    assert_eq!(read(&uploaded)[0], 8);
    let second_source = GpuVolumeSource::new(&device, &queue, volume, || false).unwrap();
    assert!(
        GpuLabels::upload(second_source, changed.clone(), Some(base.clone()), || false).is_err()
    );
    for previous in [None, Some(base.clone())] {
        let calls = AtomicUsize::new(0);
        assert!(matches!(
            GpuLabels::upload(gpu_source.clone(), changed.clone(), previous, || calls
                .fetch_add(1, Ordering::Relaxed)
                >= 2),
            Err("Cancelled")
        ));
        assert!(read(&base).iter().all(|v| *v == 0));
        assert_eq!(read(&uploaded)[0], 8);
    }
}
