// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit, bounded offscreen readback. Never used by interactive viewports.
use crate::slice::SliceRenderer;
use orv_core::{
    scene::{ImageMetadata, Reconstruction, SceneState, VolumeMetadata, hex},
    study::Frame,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

mod readback;
mod volume;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotManifest {
    pub renderer_version: u32,
    pub scene: SceneState,
    pub scene_sha256: String,
    pub png_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<ImageMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<ProjectionMetadata>,
    /// The PNG contains the rendered scalar image, without UI text/annotations.
    pub includes_ui_overlays: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionMetadata {
    pub source: VolumeMetadata,
    pub nominal_grid_step: f64,
    pub maximum_intervals: u32,
}

pub struct Snapshot {
    pub png: Vec<u8>,
    pub manifest: SnapshotManifest,
}

/// Create and call on a worker. Reuse the device; serialize captures per instance.
pub struct SnapshotRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    engine: SliceRenderer,
    volume: Option<(Arc<orv_core::volume::Volume>, crate::volume::VolumeRenderer)>,
}

impl SnapshotRenderer {
    pub async fn new() -> Result<Self, &'static str> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: crate::configured_backends()?,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance
            .request_adapter(&Default::default())
            .await
            .map_err(|_| "No supported GPU is available for snapshots")?;
        let (device, queue) = adapter
            .request_device(&crate::device_descriptor(&adapter))
            .await
            .map_err(|_| "Could not create the snapshot GPU device")?;
        Ok(Self::with_device(device, queue))
    }

    /// Share an existing native GPU device. Construct and use on an export worker.
    pub fn with_device(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let engine = SliceRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm);
        Self {
            device,
            queue,
            engine,
            volume: None,
        }
    }

    /// The caller must resolve/authorize the source before calling and recheck
    /// consent before returning these bytes. Cancellation discards partial work.
    pub fn capture(
        &mut self,
        frame: &Frame,
        scene: &SceneState,
        cancelled: impl Fn() -> bool,
    ) -> Result<Snapshot, &'static str> {
        scene.validate()?;
        if matches!(scene.reconstruction, Some(Reconstruction::Volume(_))) {
            return Err("A volume scene requires the volume snapshot path");
        }
        if cancelled() {
            return Err("Cancelled");
        }
        let [width, height] = scene.output_size;
        self.engine.upload(&self.device, &self.queue, frame)?;
        self.engine.set_view(
            &self.queue,
            frame,
            &scene.view,
            width as f32 / height as f32,
        )?;
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("explicit snapshot"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let target_view = target.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("snapshot pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.engine.paint(&mut pass);
        }
        self.queue.submit([encoder.finish()]);
        let png = readback::capture(
            &self.device,
            &self.queue,
            &target,
            scene.output_size,
            &cancelled,
        )?;
        finish(scene, Some(ImageMetadata::from_frame(frame)), None, png)
    }
}

fn finish(
    scene: &SceneState,
    image: Option<ImageMetadata>,
    volume: Option<ProjectionMetadata>,
    png: Vec<u8>,
) -> Result<Snapshot, &'static str> {
    let manifest = SnapshotManifest {
        renderer_version: 6, // Source-bound grayscale functions, tables and intensity context.
        scene: scene.clone(),
        scene_sha256: scene.hash()?,
        png_sha256: hex(&Sha256::digest(&png)),
        image,
        volume,
        includes_ui_overlays: false,
    };
    Ok(Snapshot { png, manifest })
}
