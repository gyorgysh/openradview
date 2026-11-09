// SPDX-License-Identifier: GPL-3.0-or-later
//! Immutable categorical GPU revisions. Creation/uploads belong on a worker.
use crate::{
    volume::{allocation, flush_upload},
    volume_source::GpuVolumeSource,
};
use orv_core::{
    labels::{LABEL_TILE_SIDE, Labels},
    resources::MemoryBudget,
};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadKind {
    Full,
    Delta,
    Reuse,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UploadStatistics {
    pub kind: UploadKind,
    pub uploaded_tiles: usize,
    pub uploaded_bytes: u64,
    /// A delta currently copies the whole old GPU grid before tile uploads.
    pub copied_bytes: u64,
}
pub struct GpuLabels {
    source: Arc<GpuVolumeSource>,
    labels: Arc<Labels>,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    _reservation: allocation::GpuReservation,
}
impl GpuLabels {
    /// Build a complete immutable version on the source's own device/queue.
    /// Cancellation never changes a previous texture. Retain the returned Arc
    /// through submission/presentation of every command that uses its view.
    pub fn upload(
        source: Arc<GpuVolumeSource>,
        labels: Arc<Labels>,
        previous: Option<Arc<Self>>,
        cancelled: impl Fn() -> bool,
    ) -> Result<(Arc<Self>, UploadStatistics), &'static str> {
        if !Arc::ptr_eq(&labels.source_arc(), source.volume()) {
            return Err("Label map and GPU source differ");
        }
        if previous
            .as_ref()
            .is_some_and(|previous| !Arc::ptr_eq(&previous.source, &source))
        {
            return Err("Previous label upload belongs to another GPU source");
        }
        if cancelled() {
            return Err("Cancelled");
        }
        if let Some(previous) = previous.as_ref()
            && labels.object_id() == previous.labels.object_id()
            && labels.revision() == previous.labels.revision()
        {
            return Ok((
                previous.clone(),
                UploadStatistics {
                    kind: UploadKind::Reuse,
                    uploaded_tiles: 0,
                    uploaded_bytes: 0,
                    copied_bytes: 0,
                },
            ));
        }
        let size = source.volume().dimensions();
        let bytes = size
            .into_iter()
            .try_fold(1u64, |n, side| n.checked_mul(side as u64))
            .ok_or("GPU label grid size overflow")?;
        let device = source.device();
        let queue = source.queue();
        // Queue uploads may pad each tile row to the copy alignment internally.
        // Count padded rows when bounding pending staging, not only source bytes.
        let staging_total = size[0].div_ceil(LABEL_TILE_SIDE) as u64
            * u64::from(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * size[1] as u64
            * size[2] as u64;
        let staging_limit = staging_total.min(16 * 1024 * 1024);
        let maximum_tile_staging =
            u64::from(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * LABEL_TILE_SIDE.pow(2) as u64;
        let reservation = allocation::reserve_bytes(
            bytes + staging_limit + maximum_tile_staging + 65536,
            device,
            queue,
        )?;
        let extent = wgpu::Extent3d {
            width: size[0] as u32,
            height: size[1] as u32,
            depth_or_array_layers: size[2] as u32,
        };
        let texture = allocation::texture(
            device,
            &wgpu::TextureDescriptor {
                label: Some("immutable source-grid manual labels"),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D3,
                format: wgpu::TextureFormat::R8Uint,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
        )?;
        let related = previous
            .as_ref()
            .filter(|old| old.labels.object_id() == labels.object_id());
        let mut stats = UploadStatistics {
            kind: if related.is_some() {
                UploadKind::Delta
            } else {
                UploadKind::Full
            },
            uploaded_tiles: 0,
            uploaded_bytes: 0,
            copied_bytes: 0,
        };
        if let Some(previous) = related {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("retain prior categorical cells"),
            });
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &previous.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                extent,
            );
            // write_texture staging is processed before the next submission's
            // commands. Submit this copy first so it cannot overwrite the edits.
            queue.submit([encoder.finish()]);
            stats.copied_bytes = bytes;
        }
        let capacity: usize = size
            .into_iter()
            .map(|side| side.min(LABEL_TILE_SIDE))
            .product();
        let _cpu_scratch = MemoryBudget::volumes()
            .reserve(capacity as u64)
            .ok_or("Not enough memory for label upload staging")?;
        let mut scratch = Vec::new();
        scratch
            .try_reserve_exact(capacity)
            .map_err(|_| "Not enough memory for label upload staging")?;
        scratch.resize(capacity, 0);
        let changed = related
            .map(|old| labels.changed_tiles_since(&old.labels))
            .transpose()?;
        let all = related.is_none().then(|| labels.tiles());
        let tiles = changed
            .into_iter()
            .flatten()
            .chain(all.into_iter().flatten());
        let mut pending = 0u64;
        for tile in tiles {
            if cancelled() {
                return Err("Cancelled");
            }
            let [minimum, maximum] = tile.bounds;
            let dimensions: [usize; 3] = std::array::from_fn(|a| maximum[a] - minimum[a]);
            let count: usize = dimensions.into_iter().product();
            labels.copy_tile(tile, &mut scratch[..count])?;
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: minimum[0] as u32,
                        y: minimum[1] as u32,
                        z: minimum[2] as u32,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &scratch[..count],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(dimensions[0] as u32),
                    rows_per_image: Some(dimensions[1] as u32),
                },
                wgpu::Extent3d {
                    width: dimensions[0] as u32,
                    height: dimensions[1] as u32,
                    depth_or_array_layers: dimensions[2] as u32,
                },
            );
            stats.uploaded_tiles += 1;
            stats.uploaded_bytes += count as u64;
            pending += u64::from(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
                * dimensions[1] as u64
                * dimensions[2] as u64;
            if pending >= staging_limit {
                flush_upload(device, queue, &cancelled)?;
                pending = 0;
            }
        }
        // Also fence an all-zero tile difference after a related GPU copy.
        flush_upload(device, queue, &cancelled)?;
        if cancelled() {
            return Err("Cancelled");
        }
        let view = texture.create_view(&Default::default());
        Ok((
            Arc::new(Self {
                source,
                labels,
                texture,
                view,
                _reservation: reservation,
            }),
            stats,
        ))
    }
    pub fn source(&self) -> &Arc<GpuVolumeSource> {
        &self.source
    }
    pub fn labels(&self) -> &Arc<Labels> {
        &self.labels
    }
    /// Categorical integer sampling only; retain this GpuLabels through submission.
    pub fn texture_view(&self) -> &wgpu::TextureView {
        &self.view
    }
}

#[cfg(all(test, feature = "gpu-tests"))]
mod tests;
