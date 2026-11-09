// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
pub(super) fn source(
    pool: &GpuBrickAtlasPool,
    slots: &[Option<usize>],
    previous: &[Option<usize>],
    texture: &wgpu::Texture,
    validity: &wgpu::Texture,
    cancelled: &impl Fn() -> bool,
) -> Result<(usize, u64), &'static str> {
    let source = &pool.source;
    let device = source.device();
    let queue = source.queue();
    let masked = source.source.has_missing_scalar();
    let needs_upload = slots
        .iter()
        .flatten()
        .any(|i| !previous.contains(&Some(*i)));
    if !needs_upload {
        return Ok((0, 0));
    }
    let [mw, mh, md] = pool.stored.map(|v| v as usize);
    let count = mw * mh * md;
    let bytes = count as u64 * if masked { 5 } else { 4 };
    if bytes > 16 * 1024 * 1024 {
        return Err("Atlas upload scratch exceeds its bound");
    }
    let _cpu = MemoryBudget::volumes()
        .reserve(bytes)
        .ok_or("Not enough memory for bounded atlas upload scratch")?;
    let padded = ((mw * 4).next_multiple_of(256) * mh * md
        + if masked {
            mw.next_multiple_of(256) * mh * md
        } else {
            0
        }) as u64;
    let _staging = allocation::reserve_bytes(16 * 1024 * 1024 + padded, device, queue)?;
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| "Not enough memory for atlas scalar upload")?;
    let mut known = Vec::new();
    if masked {
        known
            .try_reserve_exact(count)
            .map_err(|_| "Not enough memory for atlas validity upload")?;
    }
    let width = source.source.dimensions()[0];
    let mut pending = 0;
    let mut uploaded = 0;
    let mut uploaded_bytes = 0;
    for (slot, index) in slots.iter().enumerate() {
        let Some(index) = *index else { continue };
        if previous.contains(&Some(index)) {
            continue;
        }
        if cancelled() {
            return Err("Cancelled");
        }
        let b = source
            .layout
            .brick(index)
            .ok_or("Invalid atlas source page")?;
        let [w, h, d] = b.size;
        values.clear();
        known.clear();
        for z in b.origin[2]..b.origin[2] + d {
            let frame = &source.source.frames()[z];
            for y in b.origin[1]..b.origin[1] + h {
                if cancelled() {
                    return Err("Cancelled");
                }
                let start = y * width + b.origin[0];
                let row = &frame.pixels[start..start + w];
                if row.iter().any(|v| !v.is_finite() || v.abs() > 1e20) {
                    return Err("Atlas source values exceed GPU arithmetic limits");
                }
                values.extend_from_slice(row);
                if masked {
                    known.extend((start..start + w).map(|i| u8::from(frame.has_sample(i))));
                }
            }
        }
        let size = wgpu::Extent3d {
            width: w as u32,
            height: h as u32,
            depth_or_array_layers: d as u32,
        };
        let origin = pool.origin(slot);
        let mut target = texture.as_image_copy();
        target.origin = origin;
        queue.write_texture(
            target,
            bytemuck::cast_slice(&values),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w as u32 * 4),
                rows_per_image: Some(h as u32),
            },
            size,
        );
        if masked {
            let mut target = validity.as_image_copy();
            target.origin = origin;
            queue.write_texture(
                target,
                &known,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w as u32),
                    rows_per_image: Some(h as u32),
                },
                size,
            );
        }
        uploaded += 1;
        uploaded_bytes += (w * h * d) as u64 * if masked { 5 } else { 4 };
        pending += ((w * 4).next_multiple_of(256) * h * d
            + if masked {
                w.next_multiple_of(256) * h * d
            } else {
                0
            }) as u64;
        if pending >= 16 * 1024 * 1024 {
            flush_upload(device, queue, cancelled)?;
            pending = 0;
        }
    }
    if pending != 0 {
        flush_upload(device, queue, cancelled)?;
    }
    Ok((uploaded, uploaded_bytes))
}
