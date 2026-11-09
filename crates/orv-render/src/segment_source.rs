// SPDX-License-Identifier: GPL-3.0-or-later
//! Immutable supplied values on compact known planes, retaining the original owner.
use crate::{
    volume::{allocation, flush_upload},
    volume_source::GpuVolumeSource,
};
use orv_core::{
    resources::MemoryBudget,
    segments::{SegmentEncoding, Segments},
};
use std::sync::Arc;

const ROWS: usize = 32;
const STAGING: u64 = 16 * 1024 * 1024;
pub const UNKNOWN_PLANE: u32 = u32::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SegmentUploadStatistics {
    pub reused: bool,
    pub supplied_planes: usize,
    pub stored_sample_bytes: u64,
    pub uploaded_sample_bytes: u64,
    pub uploaded_address_bytes: u64,
}
pub struct GpuSegment {
    source: Arc<GpuVolumeSource>,
    segments: Arc<Segments>,
    number: u16,
    samples: wgpu::Texture,
    addresses: wgpu::Texture,
    sample_view: wgpu::TextureView,
    address_view: wgpu::TextureView,
    statistics: SegmentUploadStatistics,
    _reservation: allocation::GpuReservation,
}
fn zeros(count: usize) -> Result<Vec<u8>, &'static str> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(count)
        .map_err(|_| "Not enough memory for supplied GPU staging")?;
    bytes.resize(count, 0);
    Ok(bytes)
}
impl GpuSegment {
    /// Worker-only. A complete immutable upload on the source's own device/queue.
    /// Reuse requires the same source, object, revision and selected ID. A failure
    /// never changes an earlier upload. Retain returned owners through GPU use.
    pub fn upload(
        source: Arc<GpuVolumeSource>,
        segments: Arc<Segments>,
        number: u16,
        previous: Option<Arc<Self>>,
        cancelled: impl Fn() -> bool,
    ) -> Result<(Arc<Self>, SegmentUploadStatistics), &'static str> {
        if !Arc::ptr_eq(&segments.source_arc(), source.volume()) {
            return Err("Supplied masks and GPU source differ");
        }
        if previous
            .as_ref()
            .is_some_and(|old| !Arc::ptr_eq(&old.source, &source))
        {
            return Err("Previous supplied upload belongs to another GPU source");
        }
        let mask = segments.mask(number).ok_or("Unknown supplied segment ID")?;
        if cancelled() {
            return Err("Cancelled");
        }
        if let Some(old) = previous.as_ref()
            && old.number == number
            && old.segments.object_id() == segments.object_id()
            && old.segments.revision() == segments.revision()
        {
            let mut stats = old.statistics;
            stats.reused = true;
            stats.uploaded_sample_bytes = 0;
            stats.uploaded_address_bytes = 0;
            return Ok((old.clone(), stats));
        }
        let [w, h, d] = source.volume().dimensions();
        let supplied = segments
            .infos()
            .find(|i| i.number == number)
            .ok_or("Unknown supplied segment ID")?
            .supplied_planes;
        let per_word = match segments.encoding() {
            SegmentEncoding::Binary => 32,
            SegmentEncoding::Fractional(_) => 4,
        };
        let row_bytes = w.div_ceil(per_word) * 4;
        let batch_rows = if supplied == 0 { 0 } else { h.min(ROWS) };
        let batch_bytes = row_bytes * batch_rows;
        let alignment = u64::from(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let padded_row = (row_bytes as u64).div_ceil(alignment) * alignment;
        let map_staging = d as u64 * alignment;
        let staging_limit = (padded_row * h as u64 * supplied as u64 + map_staging).min(STAGING);
        let extent = if supplied == 0 {
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            }
        } else {
            wgpu::Extent3d {
                width: (row_bytes / 4) as u32,
                height: h as u32,
                depth_or_array_layers: supplied as u32,
            }
        };
        let sample_bytes = u64::from(extent.width)
            * u64::from(extent.height)
            * u64::from(extent.depth_or_array_layers)
            * 4;
        let device = source.device();
        let queue = source.queue();
        let reservation = allocation::reserve_bytes(
            sample_bytes
                + d as u64 * 4
                + staging_limit
                + (padded_row * batch_rows as u64).max(map_staging)
                + 65536,
            device,
            queue,
        )?;
        let raw_bytes = if supplied == 0 { 0 } else { w };
        let _cpu = MemoryBudget::volumes()
            .reserve((raw_bytes + batch_bytes + d * 4) as u64)
            .ok_or("Not enough memory for supplied GPU staging")?;
        let mut raw = zeros(raw_bytes)?;
        let mut packed = zeros(batch_bytes)?;
        let mut map = zeros(d * 4)?;
        map.fill(255);
        let texture = |label, size| {
            allocation::texture(
                device,
                &wgpu::TextureDescriptor {
                    label: Some(label),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D3,
                    format: wgpu::TextureFormat::R32Uint,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_SRC
                        | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                },
            )
        };
        let samples = texture("compact original supplied values", extent)?;
        let addresses = texture(
            "known source-plane addresses",
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: d as u32,
            },
        )?;
        let mut compact = 0u32;
        let mut pending = 0;
        for z in 0..d {
            if cancelled() {
                return Err("Cancelled");
            }
            if !segments.plane_is_supplied(number, z) {
                continue;
            }
            map[z * 4..z * 4 + 4].copy_from_slice(&compact.to_le_bytes());
            for first in (0..h).step_by(ROWS) {
                if cancelled() {
                    return Err("Cancelled");
                }
                let rows = (h - first).min(ROWS);
                packed[..rows * row_bytes].fill(0);
                for row in 0..rows {
                    if !mask.copy_raw_row(z, first + row, &mut raw)? {
                        return Err("Supplied coverage changed while packing");
                    }
                    let output = &mut packed[row * row_bytes..(row + 1) * row_bytes];
                    if per_word == 4 {
                        output[..w].copy_from_slice(&raw);
                    } else {
                        for (x, &value) in raw.iter().enumerate() {
                            output[x / 8] |= value << (x % 8);
                        }
                    }
                }
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &samples,
                        mip_level: 0,
                        origin: wgpu::Origin3d {
                            x: 0,
                            y: first as u32,
                            z: compact,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    &packed[..rows * row_bytes],
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(row_bytes as u32),
                        rows_per_image: Some(rows as u32),
                    },
                    wgpu::Extent3d {
                        width: (row_bytes / 4) as u32,
                        height: rows as u32,
                        depth_or_array_layers: 1,
                    },
                );
                pending += padded_row * rows as u64;
                if pending >= staging_limit {
                    flush_upload(device, queue, &cancelled)?;
                    pending = 0;
                }
            }
            compact += 1;
        }
        if compact as usize != supplied {
            return Err("Supplied plane count changed while packing");
        }
        queue.write_texture(
            addresses.as_image_copy(),
            &map,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: d as u32,
            },
        );
        flush_upload(device, queue, &cancelled)?;
        if cancelled() {
            return Err("Cancelled");
        }
        let sample_view = samples.create_view(&Default::default());
        let address_view = addresses.create_view(&Default::default());
        let statistics = SegmentUploadStatistics {
            reused: false,
            supplied_planes: supplied,
            stored_sample_bytes: sample_bytes,
            uploaded_sample_bytes: row_bytes as u64 * h as u64 * supplied as u64,
            uploaded_address_bytes: d as u64 * 4,
        };
        Ok((
            Arc::new(Self {
                source,
                segments,
                number,
                samples,
                addresses,
                sample_view,
                address_view,
                statistics,
                _reservation: reservation,
            }),
            statistics,
        ))
    }
    pub fn source(&self) -> &Arc<GpuVolumeSource> {
        &self.source
    }
    pub fn segments(&self) -> &Arc<Segments> {
        &self.segments
    }
    pub fn number(&self) -> u16 {
        self.number
    }
    pub fn encoding(&self) -> SegmentEncoding {
        self.segments.encoding()
    }
    /// Packed little-endian words: 32 binary cells or four original fractional
    /// bytes per texel, row padding zero. Depth is compact supplied-plane order.
    pub fn packed_texture(&self) -> &wgpu::Texture {
        &self.samples
    }
    /// Source-z -> compact-z; UNKNOWN_PLANE means omitted, not known zero.
    pub fn address_texture(&self) -> &wgpu::Texture {
        &self.addresses
    }
    pub(crate) fn sample_view(&self) -> &wgpu::TextureView {
        &self.sample_view
    }
    pub(crate) fn address_view(&self) -> &wgpu::TextureView {
        &self.address_view
    }
}

#[cfg(all(test, feature = "gpu-tests"))]
mod tests;
