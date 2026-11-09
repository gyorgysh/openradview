// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use orv_core::{oblique::ObliquePlane, slab::Slab, volume::ReformatGrid};
use orv_render::{reslice::GpuReslicer, volume_source::GpuVolumeSource};

#[derive(Default)]
pub(super) struct Gpu {
    context: Option<(wgpu::Device, wgpu::Queue)>,
    renderer: Option<GpuReslicer>,
    refused: bool,
}
impl Gpu {
    pub fn release(&mut self) {
        self.renderer = None;
    }
    pub fn offer(&mut self, context: (wgpu::Device, wgpu::Queue)) {
        self.context = Some(context);
    }
    fn prepare(&mut self, volume: &Arc<Volume>, cancel: &impl Fn() -> bool) {
        if self.renderer.is_some() || self.refused || volume.is_color() {
            return;
        }
        let Some((device, queue)) = &self.context else {
            return;
        };
        let result = orv_render::volume::check_limits(&device.limits())
            .and_then(|()| GpuVolumeSource::new(device, queue, volume.clone(), cancel))
            .and_then(GpuReslicer::new);
        match result {
            Ok(renderer) => self.renderer = Some(renderer),
            Err("Cancelled") => (),
            Err(_) => self.refused = true,
        }
    }
}
struct CachedPlane {
    coordinate: f64,
    side: u32,
    slab: Option<Slab>,
    frame: Arc<Frame>,
    gpu: Option<Arc<GpuReformatImage>>,
}
// One worker owns one source. Only the normal coordinate changes a plane's pixels.
#[derive(Default)]
pub(super) struct PlaneCache {
    gpu: bool,
    planes: [Option<CachedPlane>; 3],
}
pub(super) fn render(
    volume: &Arc<Volume>,
    request: &Request,
    preview: bool,
    cache: &mut PlaneCache,
    gpu: &mut Gpu,
    cancel: impl Fn() -> bool,
) -> Result<MprSnapshot, GeometryError> {
    if cancel() {
        return Err(GeometryError::Cancelled);
    }
    if !request.canonical {
        gpu.prepare(volume, &cancel);
    }
    if cancel() {
        return Err(GeometryError::Cancelled);
    }
    let renderer = if request.canonical {
        None
    } else {
        gpu.renderer.as_mut()
    };
    let accelerated = renderer.is_some();
    if cache.gpu != accelerated {
        cache.planes.fill_with(|| None);
        cache.gpu = accelerated;
    }
    let mut grids: [Option<ReformatGrid>; 3] = std::array::from_fn(|_| None);
    for (index, plane) in PLANES.into_iter().enumerate() {
        let coordinate = request.point[[2, 1, 0][index]];
        if cache.planes[index].as_ref().is_some_and(|old| {
            old.coordinate == coordinate
                && old.side == request.reformat_side
                && old.slab == request.slab
        }) {
            continue;
        }
        grids[index] = Some(if let Some(slab) = request.slab {
            volume.oblique_grid(
                &ObliquePlane::anatomical(plane, request.point)?,
                request.reformat_side,
                Some(slab),
            )?
        } else {
            volume.reformat_grid(plane, request.point, request.reformat_side)?
        });
    }
    let mut completed = if let Some(renderer) = renderer {
        // Unsupported devices, precision limits or admission retain the same
        // requested CPU grid. A rejected GPU result never substitutes pixels.
        match renderer.render_frames(grids.each_ref().map(Option::as_ref), &cancel) {
            Ok(images) => images,
            Err("Cancelled") => return Err(GeometryError::Cancelled),
            Err(_) => {
                gpu.refused = true;
                gpu.renderer = None;
                // Retry this whole set once in CPU mode, retiring any cached
                // GPU planes. Settling then retains these same complete pixels.
                return render(volume, request, preview, cache, gpu, cancel);
            }
        }
    } else {
        std::array::from_fn(|_| None)
    };
    for (index, grid) in grids.iter().enumerate() {
        if cancel() {
            return Err(GeometryError::Cancelled);
        }
        let Some(grid) = grid else {
            continue;
        };
        let image = completed[index].take();
        let frame = if let Some(image) = &image {
            image.frame().clone()
        } else {
            volume.resample_grid(grid, &cancel)?
        };
        cache.planes[index] = Some(CachedPlane {
            coordinate: request.point[[2, 1, 0][index]],
            side: request.reformat_side,
            slab: request.slab,
            frame,
            gpu: image,
        });
    }
    let images = cache
        .planes
        .iter()
        .map(|plane| {
            plane
                .as_ref()
                .map(|p| p.frame.clone())
                .ok_or(GeometryError::Dimensions)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(MprSnapshot {
        volume: volume.clone(),
        point: request.point,
        images: images.try_into().map_err(|_| GeometryError::Dimensions)?,
        gpu_images: std::array::from_fn(|i| cache.planes[i].as_ref().and_then(|p| p.gpu.clone())),
        generation: request.generation,
        preview,
        reformat_side: request.reformat_side,
        canonical_requested: request.canonical,
    })
}
