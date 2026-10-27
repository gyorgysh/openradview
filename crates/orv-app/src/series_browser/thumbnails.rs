// SPDX-License-Identifier: GPL-3.0-or-later
//! Visible-row previews only. No files, persistent catalogue or idle work.
use eframe::egui;
use orv_core::{
    resources::{MemoryBudget, Reservation},
    study::{Frame, Study},
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

const SIDE: usize = 64;
const BYTES: u64 = (SIDE * SIDE * 4) as u64;
const CAPACITY: usize = 32;
struct Preview {
    index: usize,
    image: Option<(egui::ColorImage, Reservation)>,
}
#[derive(Default)]
struct Work {
    request: Option<(u64, Vec<usize>)>,
    result: Vec<Preview>,
}
#[derive(Default)]
struct Shared {
    work: Mutex<Work>,
    changed: Condvar,
    cancel: AtomicBool,
    generation: AtomicU64,
}
struct Worker(Arc<Shared>);
impl Worker {
    fn start(study: Arc<Study>, ctx: egui::Context) -> Option<Self> {
        let shared = Arc::new(Shared::default());
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("orv-series-previews".into())
            .spawn(move || {
                loop {
                    let mut work = worker.work.lock().unwrap_or_else(|p| p.into_inner());
                    while work.request.is_none() && !worker.cancel.load(Ordering::Acquire) {
                        work = worker.changed.wait(work).unwrap_or_else(|p| p.into_inner());
                    }
                    if worker.cancel.load(Ordering::Acquire) {
                        return;
                    }
                    let Some((generation, rows)) = work.request.take() else {
                        continue;
                    };
                    drop(work);
                    let cancelled = || {
                        worker.cancel.load(Ordering::Acquire)
                            || worker.generation.load(Ordering::Acquire) != generation
                    };
                    for index in rows {
                        if cancelled() {
                            break;
                        }
                        orv_core::resources::refresh_memory_policy();
                        let image = MemoryBudget::decoded_cache().reserve(BYTES * 2).and_then(
                            |reservation| {
                                let series = study.series.get(index)?;
                                let frame = series
                                    .read_frame(series.frame_count() / 2, cancelled)
                                    .ok()?;
                                Some((make_image(&frame, cancelled)?, reservation))
                            },
                        );
                        let mut work = worker.work.lock().unwrap_or_else(|p| p.into_inner());
                        if cancelled() {
                            break;
                        }
                        if work.result.len() < CAPACITY {
                            work.result.push(Preview { index, image });
                        }
                        drop(work);
                        ctx.request_repaint();
                    }
                }
            })
            .ok()?;
        Some(Self(shared))
    }
    fn request(&self, mut rows: Vec<usize>) {
        rows.truncate(CAPACITY);
        let mut work = self.0.work.lock().unwrap_or_else(|p| p.into_inner());
        let generation = self
            .0
            .generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        work.request = (!rows.is_empty()).then_some((generation, rows));
        work.result.clear();
        self.0.changed.notify_one();
    }
    fn take(&self) -> Vec<Preview> {
        self.0
            .work
            .try_lock()
            .map(|mut work| std::mem::take(&mut work.result))
            .unwrap_or_default()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let _work = self.0.work.lock().unwrap_or_else(|p| p.into_inner());
        self.0.cancel.store(true, Ordering::Release);
        self.0.changed.notify_all();
    }
}
struct Entry {
    texture: Option<egui::TextureHandle>,
    _ram: Option<Reservation>,
    _gpu: Option<orv_render::GpuReservation>,
    used: u64,
}
#[derive(Default)]
pub(super) struct Thumbnails {
    worker: Option<Worker>,
    entries: BTreeMap<usize, Entry>,
    wanted: Vec<usize>,
    clock: u64,
}
impl Thumbnails {
    pub fn start(study: Arc<Study>, ctx: egui::Context) -> Self {
        Self {
            worker: Worker::start(study, ctx),
            ..Default::default()
        }
    }
    pub fn poll(&mut self, ui: &egui::Ui, render: Option<&eframe::egui_wgpu::RenderState>) {
        let results = self.worker.as_ref().map(Worker::take).unwrap_or_default();
        for preview in results {
            if !self.wanted.contains(&preview.index) {
                continue;
            }
            if self.entries.len() == CAPACITY
                && !self.entries.contains_key(&preview.index)
                && let Some(index) = self
                    .entries
                    .iter()
                    .filter(|(index, _)| !self.wanted.contains(index))
                    .min_by_key(|(_, entry)| entry.used)
                    .map(|(index, _)| *index)
            {
                self.entries.remove(&index);
            }
            if self.entries.len() >= CAPACITY {
                continue;
            }
            let mut entry = Entry {
                texture: None,
                _ram: None,
                _gpu: None,
                used: self.clock,
            };
            if let Some((image, ram)) = preview.image
                && let Some(render) = render
                && let Ok(gpu) =
                    orv_render::GpuReservation::new(BYTES * 3, &render.device, &render.queue)
            {
                entry.texture = Some(ui.ctx().load_texture(
                    "local series preview",
                    image,
                    egui::TextureOptions::LINEAR,
                ));
                entry._ram = Some(ram);
                entry._gpu = Some(gpu);
            }
            self.entries.insert(preview.index, entry);
        }
    }
    pub fn texture(&self, index: usize) -> Option<egui::TextureId> {
        self.entries
            .get(&index)?
            .texture
            .as_ref()
            .map(egui::TextureHandle::id)
    }
    pub fn visible(&mut self, rows: Vec<usize>) {
        let rows: Vec<_> = rows.into_iter().take(CAPACITY).collect();
        self.clock = self.clock.wrapping_add(1);
        for index in &rows {
            if let Some(entry) = self.entries.get_mut(index) {
                entry.used = self.clock;
            }
        }
        if self.wanted != rows {
            self.wanted = rows;
            if let Some(worker) = &self.worker {
                worker.request(
                    self.wanted
                        .iter()
                        .filter(|index| !self.entries.contains_key(index))
                        .copied()
                        .collect(),
                );
            }
        }
    }
}

fn make_image(frame: &Frame, cancelled: impl Fn() -> bool) -> Option<egui::ColorImage> {
    if frame.width == 0 || frame.height == 0 {
        return None;
    }
    let aspect = frame.aspect_ratio();
    if !aspect.is_finite() || aspect <= 0. {
        return None;
    }
    let width = if aspect >= 1. {
        SIDE
    } else {
        ((SIDE as f32 * aspect).round() as usize).clamp(1, SIDE)
    };
    let height = if aspect <= 1. {
        SIDE
    } else {
        ((SIDE as f32 / aspect).round() as usize).clamp(1, SIDE)
    };
    let mut image = egui::ColorImage::filled([SIDE, SIDE], egui::Color32::from_gray(10));
    for y in 0..height {
        if cancelled() {
            return None;
        }
        for x in 0..width {
            let sx = ((2 * x + 1) as u64 * u64::from(frame.width) / (2 * width) as u64) as usize;
            let sy = ((2 * y + 1) as u64 * u64::from(frame.height) / (2 * height) as u64) as usize;
            let index = sy * frame.width as usize + sx;
            if !frame.has_sample(index) {
                continue;
            }
            let color = if let Some(rgba) = &frame.rgba {
                let pixel = rgba.get(index * 4..index * 4 + 4)?;
                egui::Color32::from_rgb(pixel[0], pixel[1], pixel[2])
            } else {
                let value = frame.normalize(frame.default_window, *frame.pixels.get(index)?)?;
                let value = if frame.monochrome1 { 1. - value } else { value };
                egui::Color32::from_gray((value.clamp(0., 1.) * 255.).round() as u8)
            };
            image.pixels[(y + (SIDE - height) / 2) * SIDE + x + (SIDE - width) / 2] = color;
        }
    }
    Some(image)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    #[test]
    fn previews_preserve_aspect_padding_color_and_scalar_presentation() {
        let mut frame = (*orv_core::study::demo_study().series[0].frames[0]).clone();
        frame.width = 2;
        frame.height = 1;
        frame.geometry = None;
        frame.scalar = None;
        frame.pixels = Arc::from([0., 1.]);
        frame.default_window = orv_core::WindowLevel {
            width: 1.,
            center: 0.5,
            function: orv_core::VoiFunction::LinearExact,
        };
        let image = make_image(&frame, || false).unwrap();
        assert_eq!(image.pixels[0], egui::Color32::from_gray(10));
        assert_eq!(image.pixels[32 * SIDE + 16], egui::Color32::BLACK);
        assert_eq!(image.pixels[32 * SIDE + 48], egui::Color32::WHITE);
        frame.monochrome1 = true;
        let image = make_image(&frame, || false).unwrap();
        assert_eq!(image.pixels[32 * SIDE + 16], egui::Color32::WHITE);
        frame.validity = Some(Arc::from([0, 1]));
        let image = make_image(&frame, || false).unwrap();
        assert_eq!(image.pixels[32 * SIDE + 16], egui::Color32::from_gray(10));
        frame.validity = None;
        frame.pixels = Arc::from([]);
        frame.rgba = Some(Arc::from([255, 32, 64, 255, 0, 0, 0, 0]));
        let image = make_image(&frame, || false).unwrap();
        assert_eq!(
            image.pixels[32 * SIDE + 16],
            egui::Color32::from_rgb(255, 32, 64)
        );
        assert_eq!(image.pixels[32 * SIDE + 48], egui::Color32::from_gray(10));
        assert!(make_image(&frame, || true).is_none());
    }
    #[test]
    fn only_requested_rows_are_prepared_and_idle_closure_releases_the_study() {
        let mut study = orv_core::study::demo_study();
        let series = study.series[0].clone();
        study.series = vec![series; 100];
        let study = Arc::new(study);
        let weak = Arc::downgrade(&study);
        let worker = Worker::start(study, egui::Context::default()).unwrap();
        worker.request(vec![8, 42]);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut results = Vec::new();
        while results.len() < 2 {
            results.extend(worker.take());
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            results.iter().map(|p| p.index).collect::<Vec<_>>(),
            vec![8, 42]
        );
        assert!(results.iter().all(|p| p.image.is_some()));
        assert!(worker.take().is_empty());
        worker.request((0..100).collect());
        drop(results);
        drop(worker);
        while weak.upgrade().is_some() {
            assert!(
                Instant::now() < deadline,
                "Closed preview worker retained its study"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
