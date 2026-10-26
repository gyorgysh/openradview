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
