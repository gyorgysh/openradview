// SPDX-License-Identifier: GPL-3.0-or-later
//! Local-only textures bound to exact map, source and completed image identities.
use super::jobs::Worker;
use eframe::egui;
use orv_core::{
    labels::{LabelPlane, Labels},
    resources::{MemoryBudget, Reservation},
    study::Frame,
    volume::Volume,
};
use std::sync::{Arc, Weak};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum OverlayView {
    Source,
    Mpr { generation: u64, preview: bool },
    Oblique { generation: u64, preview: bool },
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Mode {
    #[default]
    Fill,
    Outline,
    Hatched,
}
#[derive(Clone, PartialEq, Eq)]
pub(super) struct Display {
    pub visible_ids: [bool; 256],
    pub mode: Mode,
}
impl Default for Display {
    fn default() -> Self {
        Self {
            visible_ids: [true; 256],
            mode: Mode::Fill,
        }
    }
}
pub(super) trait SampledPlane {
    fn dimensions(&self) -> [usize; 2];
}
impl SampledPlane for LabelPlane {
    fn dimensions(&self) -> [usize; 2] {
        LabelPlane::dimensions(self)
    }
}
pub(super) trait OverlayMap: Send + Sync + 'static {
    type Display: Clone + Eq + Send + 'static;
    type Plane: SampledPlane;
    fn source_arc(&self) -> Arc<Volume>;
    fn plane(&self, image: &Frame, cancel: &dyn Fn() -> bool) -> Result<Self::Plane, &'static str>;
    fn color(plane: &Self::Plane, index: usize, display: &Self::Display) -> egui::Color32;
}
impl OverlayMap for Labels {
    type Display = Display;
    type Plane = LabelPlane;
    fn source_arc(&self) -> Arc<Volume> {
        Labels::source_arc(self)
    }
    fn plane(&self, image: &Frame, cancel: &dyn Fn() -> bool) -> Result<LabelPlane, &'static str> {
        self.sample_plane(image, cancel)
    }
    fn color(plane: &LabelPlane, index: usize, display: &Display) -> egui::Color32 {
        let size = plane.dimensions();
        let id = plane.ids()[index];
        let visible =
            id != 0 && plane.eligibility()[index] == 1 && display.visible_ids[id as usize];
        let mark = match display.mode {
            Mode::Fill => true,
            Mode::Hatched => {
                edge(plane, index) || ((index % size[0]) + (index / size[0]) + id as usize) % 6 < 2
            }
            Mode::Outline => edge(plane, index),
        };
        if visible && mark {
            egui::Color32::from_gray(160 + (u16::from(id) * 37 % 96) as u8)
        } else {
            egui::Color32::TRANSPARENT
        }
    }
}
struct Key<M: OverlayMap> {
    map: Weak<M>,
    images: Vec<Weak<Frame>>,
    view: OverlayView,
    display: M::Display,
    side: usize,
}
impl<M: OverlayMap> Key<M> {
    fn same(&self, other: &Self) -> bool {
        self.map.ptr_eq(&other.map)
            && self.view == other.view
            && self.display == other.display
            && self.side == other.side
            && self.images.len() == other.images.len()
            && self
                .images
                .iter()
                .zip(&other.images)
                .all(|(a, b)| a.ptr_eq(b))
    }
}
struct Input<M: OverlayMap = Labels> {
    labels: Arc<M>,
    source: Arc<Volume>,
    images: Vec<Arc<Frame>>,
    display: M::Display,
    side: usize,
}
struct Prepared {
    image: egui::ColorImage,
    cpu: Reservation,
    gpu: Reservation,
}
struct Cached {
    texture: egui::TextureHandle,
    _cpu: Reservation,
    _gpu: Reservation,
}
pub(super) struct Overlays<M: OverlayMap = Labels> {
    worker: Option<Worker<Input<M>, Vec<Prepared>>>,
    request: Option<(Key<M>, u64)>,
    textures: Vec<Cached>,
    pub error: Option<String>,
}
impl<M: OverlayMap> Default for Overlays<M> {
    fn default() -> Self {
        Self {
            worker: None,
            request: None,
            textures: Vec::new(),
            error: None,
        }
    }
}
impl<M: OverlayMap> Overlays<M> {
    pub fn clear(&mut self) {
        if self.request.take().is_some()
            && let Some(worker) = &self.worker
        {
            worker.clear()
        }
        self.textures.clear();
        self.error = None;
    }
    pub fn prepare(
        &mut self,
        labels: Arc<M>,
        source: &Arc<Volume>,
        images: &[Arc<Frame>],
        view: OverlayView,
        display: &M::Display,
        ctx: &egui::Context,
    ) {
        if !Arc::ptr_eq(&labels.source_arc(), source)
            || images.is_empty()
            || images.len() > 3
            || images.iter().any(|image| image.slab.is_some())
        {
            self.clear();
            return;
        }
        let side = ctx.input(|i| i.max_texture_side);
        let key = Key {
            map: Arc::downgrade(&labels),
            images: images.iter().map(Arc::downgrade).collect(),
            view,
            display: display.clone(),
            side,
        };
        if self.request.as_ref().is_none_or(|(old, _)| !old.same(&key)) {
            self.clear();
            if self.worker.is_none() {
                match Worker::start(ctx.clone(), "orv-mask-overlays", render::<M>) {
                    Ok(worker) => self.worker = Some(worker),
                    Err(error) => {
                        self.error = Some(error);
                        self.request = Some((key, 0));
                        return;
                    }
                }
            }
            if let Some(worker) = &self.worker {
                let generation = worker.submit(Input {
                    labels,
                    source: source.clone(),
                    images: images.to_vec(),
                    display: display.clone(),
                    side,
                });
                self.request = Some((key, generation));
            }
        }
        let Some((generation, result)) = self.worker.as_ref().and_then(Worker::take) else {
            return;
        };
        if !self
            .request
            .as_ref()
            .is_some_and(|(_, expected)| *expected == generation)
        {
            return;
        }
        match result {
            Ok(images) => {
                if images.iter().any(|layer| {
                    layer
                        .image
                        .size
                        .iter()
                        .any(|v| *v > ctx.input(|i| i.max_texture_side))
                }) {
                    self.error =
                        Some("The mask plane exceeds this device's texture size limit".into());
                    return;
                }
                self.textures = images
                    .into_iter()
                    .map(|Prepared { image, cpu, gpu }| Cached {
                        texture: ctx.load_texture(
                            "local-mask-plane",
                            image,
                            egui::TextureOptions::NEAREST,
                        ),
                        _cpu: cpu,
                        _gpu: gpu,
                    })
                    .collect();
            }
            Err(error) => self.error = Some(error),
        }
    }
    pub fn texture(
        &self,
        index: usize,
        image: &Arc<Frame>,
        labels: &Arc<M>,
        display: &M::Display,
    ) -> Option<egui::TextureId> {
        let (key, _) = self.request.as_ref()?;
        if !key.map.ptr_eq(&Arc::downgrade(labels))
            || key.display != *display
            || !key.images.get(index)?.ptr_eq(&Arc::downgrade(image))
        {
            return None;
        }
        Some(self.textures.get(index)?.texture.id())
    }
}
fn render<M: OverlayMap>(
    input: Input<M>,
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<Prepared>, String> {
    orv_core::resources::refresh_memory_policy();
    if input.images.is_empty() || input.images.len() > 3 {
        return Err("Mask display needs one to three completed planes".into());
    }
    if !Arc::ptr_eq(&input.labels.source_arc(), &input.source) {
        return Err("Mask source changed".into());
    }
    let mut layers = Vec::with_capacity(input.images.len());
    for image in &input.images {
        if image.width as usize > input.side || image.height as usize > input.side {
            return Err("The mask plane exceeds this device's texture size limit".into());
        }
        let plane = input.labels.plane(image, cancel)?;
        let size = plane.dimensions();
        let count = size[0] * size[1];
        let cpu = MemoryBudget::volumes()
            .reserve(count as u64 * 4)
            .ok_or("Not enough memory for mask display pixels")?;
        let gpu = MemoryBudget::gpu()
            .reserve(count as u64 * 4)
            .ok_or("Not enough texture allowance for masks")?;
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(count)
            .map_err(|_| "Not enough memory for mask display pixels")?;
        for index in 0..count {
            if index.is_multiple_of(1024) && cancel() {
                return Err("Cancelled".into());
            }
            pixels.push(M::color(&plane, index, &input.display));
        }
        layers.push(Prepared {
            image: egui::ColorImage::new(size, pixels),
            cpu,
            gpu,
        });
    }
    if cancel() {
        return Err("Cancelled".into());
    }
    Ok(layers)
}
fn edge(plane: &LabelPlane, index: usize) -> bool {
    let [width, height] = plane.dimensions();
    let (x, y) = (index % width, index / width);
    if x == 0 || y == 0 || x + 1 == width || y + 1 == height {
        return true;
    }
    [index - 1, index + 1, index - width, index + width]
        .into_iter()
        .any(|other| plane.ids()[other] != plane.ids()[index] || plane.eligibility()[other] != 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orv_core::segments::Segments;
    use std::time::{Duration, Instant};
    fn supplied() -> Arc<Segments> {
        let mut bytes = [255; 16];
        bytes[8..].fill(0xf0);
        Arc::new(
            Segments::from_binary(
                crate::labels::tests::labels().source_arc(),
                &[u16::MAX, 2],
                &[(u16::MAX, 1), (2, 1)],
                &bytes,
                None,
                || false,
            )
            .unwrap(),
        )
    }
    #[test]
    fn supplied_overlap_visibility_and_boundaries_preserve_large_ids_and_source_counts() {
        let masks = supplied();
        let source = masks.source_arc();
        let mut display = <Segments as OverlayMap>::Display::default();
        let request = |display, image| Input {
            labels: masks.clone(),
            source: source.clone(),
            images: vec![image],
            display,
            side: 8,
        };
        let layer = render(
            request(display.clone(), source.frames()[1].clone()),
            &|| false,
        )
        .unwrap()
        .remove(0);
        assert_eq!(marked(&layer), 64);
        assert_eq!(layer.image.pixels[0], egui::Color32::from_gray(187));
        assert_eq!(layer.image.pixels[4], egui::Color32::from_gray(150));
        assert_eq!(layer.image.pixels[6], egui::Color32::WHITE);
        display.visible = 1;
        let layer = render(
            request(display.clone(), source.frames()[1].clone()),
            &|| false,
        )
        .unwrap()
        .remove(0);
        assert!(
            layer
                .image
                .pixels
                .iter()
                .all(|pixel| *pixel == egui::Color32::from_gray(187))
        );
        display.visible = 2;
        display.mode = Mode::Outline;
        let layer = render(
            request(display.clone(), source.frames()[1].clone()),
            &|| false,
        )
        .unwrap()
        .remove(0);
        assert_eq!(marked(&layer), 20);
        display.mode = Mode::Fill;
        let mut missing = (*source.frames()[1]).clone();
        let mut validity = vec![1; 64];
        validity[28] = 0;
        missing.validity = Some(validity.into());
        let layer = render(request(display.clone(), Arc::new(missing)), &|| false)
            .unwrap()
            .remove(0);
        assert_eq!(marked(&layer), 31);
        assert_eq!(layer.image.pixels[28], egui::Color32::TRANSPARENT);
        display.visible = 0;
        assert_eq!(
            marked(&render(request(display, source.frames()[1].clone()), &|| false).unwrap()[0]),
            0
        );
        assert_eq!(masks.overlap_cells(), 32);
        assert_eq!(
            masks
                .infos()
                .map(|info| info.positive_cells)
                .collect::<Vec<_>>(),
            [64, 32]
        );
    }
    #[test]
    fn supplied_cache_binds_exact_source_map_and_completed_frame() {
        let masks = supplied();
        let ctx = egui::Context::default();
        let source = masks.source_arc();
        let images = vec![source.frames()[1].clone()];
        let display = <Segments as OverlayMap>::Display::default();
        let mut overlays = Overlays::<Segments>::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            overlays.prepare(
                masks.clone(),
                &source,
                &images,
                OverlayView::Mpr {
                    generation: 7,
                    preview: false,
                },
                &display,
                &ctx,
            );
            if overlays.texture(0, &images[0], &masks, &display).is_some() {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(
            overlays
                .texture(0, &Arc::new((*images[0]).clone()), &masks, &display)
                .is_none()
        );
        assert!(
            overlays
                .texture(0, &images[0], &supplied(), &display)
                .is_none()
        );
        let different = Arc::new(Volume::new(source.frames().to_vec()).unwrap());
        overlays.prepare(
            masks.clone(),
            &different,
            &images,
            OverlayView::Source,
            &display,
            &ctx,
        );
        assert!(overlays.texture(0, &images[0], &masks, &display).is_none());
        let key = Arc::downgrade(&masks);
        drop(masks);
        assert!(key.upgrade().is_none());
    }
    fn filled() -> Arc<Labels> {
        let original = crate::labels::tests::labels();
        let mut labels = original.try_clone(|| false).unwrap();
        labels.threshold(None, [20., 20.], 1, || false).unwrap();
        Arc::new(labels)
    }
    fn input(labels: &Arc<Labels>, display: Display) -> Input {
        Input {
            source: labels.source_arc(),
            labels: labels.clone(),
            images: vec![labels.source().frames()[1].clone()],
            display,
            side: 8,
        }
    }
    fn marked(layer: &Prepared) -> usize {
        layer.image.pixels.iter().filter(|p| p.a() != 0).count()
    }
    #[test]
    fn fill_outline_hatches_and_hidden_ids_have_independent_expected_pixels() {
        let labels = filled();
        assert_eq!(
            marked(&render(input(&labels, Display::default()), &|| false).unwrap()[0]),
            64
        );
        let display = Display {
            mode: Mode::Outline,
            ..Default::default()
        };
        assert_eq!(
            marked(&render(input(&labels, display), &|| false).unwrap()[0]),
            28
        );
        let display = Display {
            mode: Mode::Hatched,
            ..Default::default()
        };
        let layers = render(input(&labels, display), &|| false).unwrap();
        assert_eq!(marked(&layers[0]), 40);
        assert_eq!(
            layers[0].image.pixels[24..32]
                .iter()
                .map(|p| p.a() != 0)
                .collect::<Vec<_>>(),
            vec![true, false, true, true, false, false, false, true]
        );
        let mut display = Display::default();
        display.visible_ids[1] = false;
        assert_eq!(
            marked(&render(input(&labels, display), &|| false).unwrap()[0]),
            0
        );
        assert_eq!(
            labels.statistics(|| false).unwrap().by_id[1].stored_voxels,
            192
        );
    }
    #[test]
    fn hatching_keeps_tiny_region_boundaries_when_the_pattern_itself_would_miss_them() {
        let original = filled();
        let frames = original
            .source()
            .frames()
            .iter()
            .map(|frame| {
                let mut frame = (**frame).clone();
                frame.width = 2;
                frame.height = 2;
                frame.pixels = vec![-100., 0., 100., -102.].into();
                Arc::new(frame)
            })
            .collect();
        let mut labels = Labels::empty(Arc::new(Volume::new(frames).unwrap())).unwrap();
        labels.threshold(None, [0., 100.], 1, || false).unwrap();
        let labels = Arc::new(labels);
        let layers = render(
            input(
                &labels,
                Display {
                    mode: Mode::Hatched,
                    ..Default::default()
                },
            ),
            &|| false,
        )
        .unwrap();
        assert_eq!(
            layers[0]
                .image
                .pixels
                .iter()
                .map(|p| p.a() != 0)
                .collect::<Vec<_>>(),
            vec![false, true, true, false]
        );
    }
    #[test]
    fn missing_output_is_transparent_and_produces_a_real_outline_boundary() {
        let labels = filled();
        let mut request = input(&labels, Display::default());
        let mut image = (*request.images[0]).clone();
        let mut mask = vec![1; 64];
        mask[27] = 0;
        image.validity = Some(mask.into());
        request.images[0] = Arc::new(image);
        let layers = render(request, &|| false).unwrap();
        assert_eq!(marked(&layers[0]), 63);
        assert_eq!(layers[0].image.pixels[27].a(), 0);
        let mut request = input(
            &labels,
            Display {
                mode: Mode::Outline,
                ..Default::default()
            },
        );
        let mut image = (*request.images[0]).clone();
        let mut mask = vec![1; 64];
        mask[27] = 0;
        image.validity = Some(mask.into());
        request.images[0] = Arc::new(image);
        let layers = render(request, &|| false).unwrap();
        assert_eq!(marked(&layers[0]), 32);
        for neighbor in [19, 26, 28, 35] {
            assert_eq!(layers[0].image.pixels[neighbor].a(), 255);
        }
    }
    #[test]
    fn device_limits_source_identity_slabs_and_cancellation_refuse_partial_layers() {
        let labels = filled();
        let mut request = input(&labels, Display::default());
        request.side = 7;
        assert!(render(request, &|| false).is_err());
        let mut request = input(&labels, Display::default());
        request.source = Arc::new(Volume::new(labels.source().frames().to_vec()).unwrap());
        assert!(render(request, &|| false).is_err());
        let mut request = input(&labels, Display::default());
        let mut image = (*request.images[0]).clone();
        image.slab = Some(orv_core::slab::Slab {
            projection: orv_core::slab::SlabProjection::Maximum,
            thickness_mm: 2.,
            intervals: 4,
        });
        request.images[0] = Arc::new(image);
        assert!(render(request, &|| false).is_err());
        assert!(render(input(&labels, Display::default()), &|| true).is_err());
    }
    #[test]
    fn cached_textures_match_exact_map_and_frame_and_keep_neither_alive() {
        let ctx = egui::Context::default();
        let labels = filled();
        let source = labels.source_arc();
        let images = vec![source.frames()[1].clone()];
        let mut overlays = Overlays::default();
        let display = Display::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            overlays.prepare(
                labels.clone(),
                &source,
                &images,
                OverlayView::Source,
                &display,
                &ctx,
            );
            if overlays.texture(0, &images[0], &labels, &display).is_some() {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        let other_image = Arc::new((*images[0]).clone());
        let other_map = Arc::new(labels.try_clone(|| false).unwrap());
        assert!(
            overlays
                .texture(0, &other_image, &labels, &display)
                .is_none()
        );
        assert!(
            overlays
                .texture(0, &images[0], &other_map, &display)
                .is_none()
        );
        let mut hidden = display.clone();
        hidden.visible_ids[1] = false;
        assert!(overlays.texture(0, &images[0], &labels, &hidden).is_none());
        let map_key = Arc::downgrade(&labels);
        let image_key = Arc::downgrade(&images[0]);
        drop(other_image);
        drop(other_map);
        drop(images);
        drop(source);
        drop(labels);
        assert!(map_key.upgrade().is_none());
        assert!(image_key.upgrade().is_none());
        overlays.clear();
        assert!(overlays.textures.is_empty());
        assert!(overlays.request.is_none());
    }
}
