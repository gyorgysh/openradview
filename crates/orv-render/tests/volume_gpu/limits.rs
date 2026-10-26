// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn oversized_source_is_rejected_before_creating_a_gpu_texture() {
    let (device, queue) = device();
    let width = device.limits().max_texture_dimension_3d + 1;
    let volume = Arc::new(
        Volume::new(
            (0..2)
                .map(|z| {
                    Arc::new(Frame {
                        validity: None,
                        color: None,
                        nifti: None,
                        rgba: None,
                        width,
                        height: 2,
                        pixels: vec![0.; width as usize * 2].into(),
                        geometry: Some(Geometry {
                            nifti_transform: None,
                            frame_of_reference: Some("synthetic-gpu-limit".into()),
                            position: [0., 0., f64::from(z)],
                            orientation: [1., 0., 0., 0., 1., 0.],
                            spacing: [1., 1.],
                        }),
                        instance: z,
                        frame_index: 0,
                        monochrome1: false,
                        default_window: WindowLevel::SOFT_TISSUE,
                    })
                })
                .collect(),
        )
        .unwrap(),
    );
    assert!(
        matches!(VolumeRenderer::new(&device,&queue,volume,wgpu::TextureFormat::Rgba8Unorm,||false),Err(message) if message.contains("3D texture limit"))
    );
}

#[test]
fn a_display_only_device_rejects_volume_before_creating_a_compute_pipeline() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: orv_render::configured_backends().unwrap(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .expect("Real GPU required");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: wgpu::Limits::downlevel_webgl2_defaults(),
        ..Default::default()
    }))
    .unwrap();
    assert!(
        matches!(VolumeRenderer::new(&device, &queue, volume(Some(100.), false),
        wgpu::TextureFormat::Rgba8Unorm, || false), Err(message) if message.contains("compute shaders"))
    );
}
