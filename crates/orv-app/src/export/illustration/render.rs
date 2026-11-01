// SPDX-License-Identifier: GPL-3.0-or-later
//! One worker renders immutable selected layers; the resulting bytes also drive preview.
use super::*;
use eframe::egui_wgpu::{Renderer, RendererOptions, ScreenDescriptor, wgpu};
use orv_core::{
    resources::{MemoryBudget, Reservation},
    scene::{ImageMetadata, SceneState, frame_pixel_hash, fresh_series_fingerprint, hex},
};
use orv_render::snapshot::{SliceImage, SnapshotRenderer};
use sha2::{Digest, Sha256};
use std::io::{self, Write};
const MIB: u64 = 1024 * 1024;
const MANIFEST_LIMIT: usize = 2 * 1024 * 1024;
const MESH_LIMIT: usize = 16 * 1024 * 1024;
#[cfg(test)]
#[path = "render_tests.rs"]
pub(super) mod tests;
fn artifact_admission(
    size: [u32; 2],
    budget: &Arc<MemoryBudget>,
) -> Result<Reservation, &'static str> {
    if size.contains(&0) || size.iter().any(|n| *n > 1024) {
        return Err("Illustration dimensions exceed the bounded pane limit");
    }
    let rgba = u64::from(size[0]) * u64::from(size[1]) * 4;
    budget
        .reserve(rgba * 4 + 3 * MIB + 16 * 1024)
        .ok_or("Not enough memory for the illustration preview")
}
struct CompositionMemory {
    _cpu: Reservation,
    _gpu: Option<Reservation>,
    _mask: Option<(Reservation, Reservation)>,
}
struct TextureUpdates(egui::TexturesDelta);
impl Drop for TextureUpdates {
    fn drop(&mut self) {
        self.0.clear();
    }
}
#[derive(Serialize)]
struct MaskRecord<'a> {
    settings: &'a crate::labels::MaskDescription,
    raster_sha256: String,
    raster_size: [usize; 2],
    sampling: &'static str,
}
#[derive(Serialize)]
struct Manifest<'a> {
    format: &'static str,
    schema_version: u32,
    renderer_version: u32,
    base_renderer_version: u32,
    app_version: &'static str,
    scene: &'a SceneState,
    image: ImageMetadata,
    png_sha256: String,
    layers: Choice,
    outline_style: Option<OutlineStyle>,
    marks: Option<&'a [crate::annotations::Outline]>,
    masks: Option<MaskRecord<'a>>,
}
#[derive(Serialize)]
struct OutlineStyle {
    stroke_pixels: f32,
    surround_pixels: f32,
    number_font: &'static str,
    number_pixels: f32,
    number_background: &'static str,
    handles: bool,
    results: bool,
}
struct LimitedBytes {
    bytes: Vec<u8>,
}
impl Write for LimitedBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MANIFEST_LIMIT.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other(
                "Illustration metadata exceeds its allowance",
            ));
        }
        self.bytes
            .try_reserve_exact(bytes.len())
            .map_err(|_| io::Error::other("Not enough memory for illustration metadata"))?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
pub(super) fn prepare(
    request: Request,
    cancel: &dyn Fn() -> bool,
) -> Result<Arc<Artifact>, String> {
    orv_core::resources::refresh_memory_policy();
    let Request { input, choice } = request;
    input.base.scene.validate()?;
    let image = input.image()?;
    input
        .outlines
        .validate(image, &input.base.scene.view, &cancel)?;
    if choice.masks && input.masks.is_none() {
        return Err("No applied mask group is available for this illustration".into());
    }
    if cancel() {
        return Err("Illustration cancelled".into());
    }
    let [width, height] = input.base.scene.output_size;
    if choice.marks {
        input.outlines.check_drawing_size(
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width as f32, height as f32)),
            image,
            &input.base.scene.view,
        )?;
    }
    let memory = artifact_admission([width, height], &MemoryBudget::volumes())?;
    let source_sha256 = verify_source(&input, choice.masks, cancel)?;
    let mut scene = input.base.scene.clone();
    scene.source_sha256 = source_sha256.clone();
    let mut renderer =
        SnapshotRenderer::with_device(input.base.device.clone(), input.base.queue.clone());
    let target = renderer.render_slice_image(image, &scene, cancel)?;
    let (_composition, mask_record) = if choice.marks || choice.masks {
        let (memory, record) = compose(&input, choice, &target, cancel)?;
        (Some(memory), record)
    } else {
        (None, None)
    };
    if cancel() {
        return Err("Illustration cancelled".into());
    }
    let png = target.png(cancel)?;
    let manifest = Manifest {
        format: "openradview_illustration",
        schema_version: 1,
        renderer_version: 1,
        base_renderer_version: orv_render::snapshot::RENDERER_VERSION,
        app_version: env!("CARGO_PKG_VERSION"),
        scene: &scene,
        image: ImageMetadata::from_frame(image),
        png_sha256: hash(&png, cancel)?,
        layers: choice,
        outline_style: choice.marks.then_some(OutlineStyle {
            stroke_pixels: 1.5,
            surround_pixels: 3.5,
            number_font: "bundled IBM Plex Mono",
            number_pixels: 11.,
            number_background: "opaque dark surround",
            handles: false,
            results: false,
        }),
        marks: choice.marks.then(|| input.outlines.marks()),
        masks: mask_record,
    };
    let mut json = LimitedBytes { bytes: Vec::new() };
    serde_json::to_writer_pretty(&mut json, &manifest)
        .map_err(|_| "Could not encode the bounded illustration metadata")?;
    if cancel() {
        return Err("Illustration cancelled".into());
    }
    let decoded = image::load_from_memory(&png)
        .map_err(|_| "Could not prepare the illustration preview")?
        .to_rgba8();
    if decoded.dimensions() != (width, height) {
        return Err("The illustration preview dimensions do not match".into());
    }
    let preview = Arc::new(egui::ColorImage::from_rgba_unmultiplied(
        [width as usize, height as usize],
        decoded.as_raw(),
    ));
    if cancel() {
        return Err("Illustration cancelled".into());
    }
    Ok(Arc::new(Artifact {
        input,
        choice,
        source_sha256,
        png,
        manifest: json.bytes,
        preview,
        size: [width, height],
        _memory: memory,
    }))
}
pub(super) fn verify_source(
    input: &FrozenInput,
    masks: bool,
    cancel: &dyn Fn() -> bool,
) -> Result<String, String> {
    let hash = fresh_series_fingerprint(&input.base.series, cancel)?;
    match &input.base.pixels {
        Pixels::Image(image) => {
            let fresh = input
                .base
                .series
                .read_fresh_frame(input.base.scene.view.slice as usize, cancel)
                .map_err(|_| "The displayed source is unavailable or changed")?;
            if image.geometry != fresh.geometry
                || ImageMetadata::from_frame(image) != ImageMetadata::from_frame(&fresh)
                || frame_pixel_hash(image, cancel)? != frame_pixel_hash(&fresh, cancel)?
            {
                return Err(
                    "The displayed image differs from its source. Reopen it before exporting"
                        .into(),
                );
            }
        }
        Pixels::Reformat(_, volume) => {
            super::super::worker::verify_volume_fresh(&input.base.series, volume, cancel)?
        }
        Pixels::Volume(_) => return Err("Illustrations currently need a thin image plane".into()),
    }
    if masks && let Some(mask) = &input.masks {
        let source = mask.source();
        let already =
            matches!(&input.base.pixels,Pixels::Reformat(_,volume) if Arc::ptr_eq(volume,&source));
        if !already {
            super::super::worker::verify_volume_fresh(&input.base.series, &source, cancel)?;
        }
    }
    Ok(hash)
}
fn hash(bytes: &[u8], cancel: &dyn Fn() -> bool) -> Result<String, String> {
    let mut digest = Sha256::new();
    for chunk in bytes.chunks(64 * 1024) {
        if cancel() {
            return Err("Illustration cancelled".into());
        }
        digest.update(chunk);
    }
    Ok(hex(&digest.finalize()))
}
fn mask_hash(image: &egui::ColorImage, cancel: &dyn Fn() -> bool) -> Result<String, String> {
    let mut digest = Sha256::new();
    let mut bytes = [0u8; 4096];
    for chunk in image.pixels.chunks(1024) {
        if cancel() {
            return Err("Illustration cancelled".into());
        }
        for (index, pixel) in chunk.iter().enumerate() {
            bytes[index * 4..index * 4 + 4].copy_from_slice(&pixel.to_array());
        }
        digest.update(&bytes[..chunk.len() * 4]);
    }
    Ok(hex(&digest.finalize()))
}
fn composition_admission(
    budget: &Arc<MemoryBudget>,
    mesh_bytes: usize,
    texture_bytes: usize,
) -> Result<Reservation, &'static str> {
    if mesh_bytes > MESH_LIMIT || texture_bytes > 32 * 1024 * 1024 {
        return Err("Illustration drawing exceeds its GPU allowance");
    }
    let bytes = 2 * (mesh_bytes as u64 + texture_bytes as u64) + MIB;
    budget
        .reserve(bytes)
        .ok_or("Not enough GPU allowance for illustration drawing")
}
fn compose<'a>(
    input: &'a FrozenInput,
    choice: Choice,
    target: &SliceImage,
    cancel: &dyn Fn() -> bool,
) -> Result<(CompositionMemory, Option<MaskRecord<'a>>), String> {
    let mut memory = CompositionMemory {
        _cpu: MemoryBudget::volumes()
            .reserve(48 * MIB)
            .ok_or("Not enough memory for illustration drawing")?,
        _gpu: None,
        _mask: None,
    };
    let device = &input.base.device;
    let queue = &input.base.queue;
    let image = input.image()?;
    let [width, height] = target.size();
    let mut pixels = if choice.masks {
        Some(
            input
                .masks
                .as_ref()
                .ok_or("No applied masks are available")?
                .prepare(image, device.limits().max_texture_dimension_2d, cancel)?,
        )
    } else {
        None
    };
    let mask_record = if let Some(pixels) = &pixels {
        Some(MaskRecord {
            settings: input
                .masks
                .as_ref()
                .ok_or("No applied masks are available")?
                .description(),
            raster_sha256: mask_hash(&pixels.image, cancel)?,
            raster_size: pixels.image.size,
            sampling: "nearest categorical cells; native visibility/style; missing samples transparent",
        })
    } else {
        None
    };
    let ctx = egui::Context::default();
    crate::theme::install(&ctx);
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width as f32, height as f32));
    let mut texture = None;
    let mut paint_error = None;
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(rect),
            max_texture_side: Some(device.limits().max_texture_dimension_2d as usize),
            ..Default::default()
        },
        |ui| {
            let ctx = ui.ctx();
            let painter = ui.painter_at(rect);
            if let Some(layer) = pixels.take() {
                let handle = ctx.load_texture(
                    "illustration-mask",
                    layer.image,
                    egui::TextureOptions::NEAREST,
                );
                memory._mask = Some((layer._cpu, layer._gpu));
                texture = Some(handle);
            }
            if let Some(handle) = &texture
                && let Some(masks) = &input.masks
                && let Err(error) =
                    masks.paint(&painter, rect, image, &input.base.scene.view, handle.id())
            {
                paint_error = Some(error);
            }
            if choice.marks {
                input
                    .outlines
                    .paint(&painter, rect, image, &input.base.scene.view);
            }
        },
    );
    let updates = TextureUpdates(std::mem::take(&mut output.textures_delta));
    if let Some(error) = paint_error {
        return Err(error.into());
    }
    if cancel() {
        return Err("Illustration cancelled".into());
    }
    let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
    let mut mesh_bytes = 0usize;
    for primitive in &primitives {
        let egui::epaint::Primitive::Mesh(mesh) = &primitive.primitive else {
            return Err("Illustrations cannot contain paint callbacks".into());
        };
        mesh_bytes = mesh_bytes
            .checked_add(
                mesh.vertices.len() * std::mem::size_of::<egui::epaint::Vertex>()
                    + mesh.indices.len() * 4,
            )
            .filter(|n| *n <= MESH_LIMIT)
            .ok_or("Illustration geometry exceeds its drawing allowance")?;
    }
    let mut texture_bytes = 0usize;
    for deltas in updates.0.set.values() {
        for delta in deltas {
            let size = delta.image.size();
            texture_bytes = texture_bytes
                .checked_add(size[0] * size[1] * 4)
                .filter(|n| *n <= 32 * 1024 * 1024)
                .ok_or("Illustration textures exceed their drawing allowance")?;
        }
    }
    // The pinned renderer retains vertex/index buffers and upload staging, and
    // texture writes can also retain staging until submission completes.
    memory._gpu = Some(composition_admission(
        &MemoryBudget::gpu(),
        mesh_bytes,
        texture_bytes,
    )?);
    let mut renderer = Renderer::new(
        device,
        wgpu::TextureFormat::Rgba8Unorm,
        RendererOptions {
            dithering: false,
            ..Default::default()
        },
    );
    for (id, deltas) in &updates.0.set {
        for delta in deltas {
            renderer.update_texture(device, queue, *id, delta);
        }
    }
    let screen = ScreenDescriptor {
        size_in_pixels: [width, height],
        pixels_per_point: output.pixels_per_point,
    };
    let mut encoder = device.create_command_encoder(&Default::default());
    let commands = renderer.update_buffers(device, queue, &mut encoder, &primitives, &screen);
    let view = target.view();
    {
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("local illustration layers"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            })
            .forget_lifetime();
        renderer.render(&mut pass, &primitives, &screen);
    }
    queue.submit(commands.into_iter().chain([encoder.finish()]));
    for id in &updates.0.free {
        renderer.free_texture(id);
    }
    drop(texture);
    Ok((memory, mask_record))
}
