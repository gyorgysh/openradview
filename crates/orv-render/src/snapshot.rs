// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit, bounded offscreen readback. Never used by interactive viewports.
use crate::slice::SliceRenderer;
use orv_core::{
    resources::{MemoryBudget, Reservation},
    scene::{ImageMetadata, Reconstruction, SceneState, VolumeMetadata, hex},
    study::Frame,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub mod mosaic;
mod readback;
mod volume;
pub const RENDERER_VERSION: u32 = 8;

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

/// A bounded worker-owned RGBA8 target, without a snapshot/consent manifest.
/// Local compositors may paint into its view before encoding. Ordinary snapshots
/// finish the untouched base target and continue to exclude all UI overlays.
pub struct SliceImage {
    texture: wgpu::Texture,
    size: [u32; 2],
    device: wgpu::Device,
    queue: wgpu::Queue,
    _gpu: Reservation,
}
impl SliceImage {
    pub fn size(&self) -> [u32; 2] {
        self.size
    }
    pub fn view(&self) -> wgpu::TextureView {
        self.texture.create_view(&Default::default())
    }
    /// Copy at most1024² RGBA8 pixels on a worker. The caller must admit retained
    /// bytes separately; the readback working reservations end when this returns.
    pub fn rgba(&self, cancelled: impl Fn() -> bool) -> Result<Vec<u8>, &'static str> {
        readback::rgba(
            &self.device,
            &self.queue,
            &self.texture,
            self.size,
            &cancelled,
        )
    }
    pub fn png(&self, cancelled: impl Fn() -> bool) -> Result<Vec<u8>, &'static str> {
        readback::capture(
            &self.device,
            &self.queue,
            &self.texture,
            self.size,
            &cancelled,
        )
    }
}

fn slice_target_admission(
    size: [u32; 2],
    budget: &Arc<MemoryBudget>,
) -> Result<Reservation, &'static str> {
    if size.contains(&0) || size.iter().any(|n| *n > 1024) {
        return Err("The slice image exceeds the bounded output dimensions");
    }
    budget
        .reserve(u64::from(size[0]) * u64::from(size[1]) * 4)
        .ok_or("Not enough GPU allowance for the offscreen image")
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
            flags: crate::instance_flags(),
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
        let image = self.render_slice_image(frame, scene, &cancelled)?;
        let png = image.png(cancelled)?;
        finish(scene, Some(ImageMetadata::from_frame(frame)), None, png)
    }

    /// Render the same validated base slice as `capture`, without labeling a
    /// subsequently composed local illustration as an image-only snapshot.
    pub fn render_slice_image(
        &mut self,
        frame: &Frame,
        scene: &SceneState,
        cancelled: impl Fn() -> bool,
    ) -> Result<SliceImage, &'static str> {
        scene.validate()?;
        if matches!(scene.reconstruction, Some(Reconstruction::Volume(_))) {
            return Err("A volume scene requires the volume snapshot path");
        }
        let expected_slab = match &scene.reconstruction {
            Some(Reconstruction::Slab(state)) => Some(state.slab),
            _ => None,
        };
        if let Some(Reconstruction::Slab(state)) = &scene.reconstruction {
            let g = frame
                .geometry
                .as_ref()
                .ok_or("Slab sampling grid is missing")?;
            let axes = [state.plane.column_direction, state.plane.row_direction].concat();
            let offset: f64 = (0..3)
                .map(|i| {
                    (g.position[i] - state.plane.center_mm[i]) * state.plane.normal_direction()[i]
                })
                .sum();
            if g.orientation
                .iter()
                .zip(axes)
                .any(|(a, b)| (*a - b).abs() > 1e-9)
                || offset.abs() > 1e-6
            {
                return Err("Snapshot slab sampling plane and scene do not match");
            }
        }
        if frame.slab != expected_slab {
            return Err("Snapshot projection and scene do not match");
        }
        if cancelled() {
            return Err("Cancelled");
        }
        let [width, height] = scene.output_size;
        let gpu = slice_target_admission(scene.output_size, &MemoryBudget::gpu())?;
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
        Ok(SliceImage {
            texture: target,
            size: scene.output_size,
            device: self.device.clone(),
            queue: self.queue.clone(),
            _gpu: gpu,
        })
    }
}

fn finish(
    scene: &SceneState,
    image: Option<ImageMetadata>,
    volume: Option<ProjectionMetadata>,
    png: Vec<u8>,
) -> Result<Snapshot, &'static str> {
    let manifest = SnapshotManifest {
        renderer_version: RENDERER_VERSION, // Physical clipping and scalar slab provenance.
        scene: scene.clone(),
        scene_sha256: scene.hash()?,
        png_sha256: hex(&Sha256::digest(&png)),
        image,
        volume,
        includes_ui_overlays: false,
    };
    Ok(Snapshot { png, manifest })
}

#[cfg(test)]
mod admission_tests {
    use super::*;
    #[test]
    fn offscreen_slice_targets_keep_existing_caps_and_release_their_admission() {
        let budget = MemoryBudget::new(31);
        assert!(slice_target_admission([4, 2], &budget).is_err());
        assert_eq!(budget.used(), 0);
        budget.set_limit(32);
        let admitted = slice_target_admission([4, 2], &budget).unwrap();
        assert_eq!(budget.used(), 32);
        assert!(slice_target_admission([1, 1], &budget).is_err());
        drop(admitted);
        assert_eq!(budget.used(), 0);
        for size in [[0, 1], [1, 0], [1025, 1], [1, 1025], [u32::MAX, u32::MAX]] {
            assert!(slice_target_admission(size, &budget).is_err());
        }
        assert_eq!(budget.used(), 0);
    }
}
