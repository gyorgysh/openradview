// SPDX-License-Identifier: GPL-3.0-or-later
use super::{ProjectionMetadata, Snapshot, SnapshotRenderer, finish, readback};
use orv_core::{
    scene::{Reconstruction, SceneState, VolumeMetadata},
    volume::Volume,
};
use std::sync::Arc;

impl SnapshotRenderer {
    pub fn capture_volume(
        &mut self,
        volume: Arc<Volume>,
        scene: &SceneState,
        cancelled: impl Fn() -> bool,
    ) -> Result<Snapshot, &'static str> {
        scene.validate()?;
        let Some(Reconstruction::Volume(state)) = &scene.reconstruction else {
            return Err(
                "Volume snapshots require explicit physical camera and rendering parameters",
            );
        };
        if cancelled() {
            return Err("Cancelled");
        }
        if !self
            .volume
            .as_ref()
            .is_some_and(|(source, _)| Arc::ptr_eq(source, &volume))
        {
            // Drop a prior GPU source before allocating its replacement.
            self.volume = None;
            let renderer = crate::volume::VolumeRenderer::new(
                &self.device,
                &self.queue,
                volume.clone(),
                wgpu::TextureFormat::Rgba8Unorm,
                &cancelled,
            )?;
            self.volume = Some((volume.clone(), renderer));
        }
        let (_, renderer) = self.volume.as_mut().ok_or("Snapshot volume unavailable")?;
        let [width, height] = scene.output_size;
        for row in (0..height).step_by(8) {
            if cancelled() {
                return Err("Cancelled");
            }
            let mut encoder = self.device.create_command_encoder(&Default::default());
            renderer.render_region(
                &self.device,
                &mut encoder,
                &scene.view,
                state,
                scene.output_size,
                [[0, row], [width, (height - row).min(8)]],
            )?;
            let submission = self.queue.submit([encoder.finish()]);
            // Bound in-flight GPU work so revoke/timeout can cancel between tiles.
            readback::wait(&self.device, Some(submission), &cancelled)?;
        }
        let target = renderer
            .output_texture()
            .ok_or("Snapshot output unavailable")?;
        let png = readback::capture(
            &self.device,
            &self.queue,
            target,
            scene.output_size,
            &cancelled,
        )?;
        finish(
            scene,
            None,
            Some(ProjectionMetadata {
                source: VolumeMetadata::from_volume(&volume),
                nominal_grid_step: state.quality.step_factor(),
                maximum_intervals: state.quality.max_steps(),
            }),
            png,
        )
    }
}
