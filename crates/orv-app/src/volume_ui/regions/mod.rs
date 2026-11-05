// SPDX-License-Identifier: GPL-3.0-or-later
//! Local coherent region frames, prepared on one coalescing worker.
mod controls;
mod picking;
mod surface;
#[cfg(test)]
mod tests;
mod worker;
use super::material::OpticalStyle;
use crate::{
    jobs::Worker,
    labels::{LabelUi, RegionSelection},
};
use eframe::{
    egui,
    egui_wgpu::{self, wgpu},
};
use orv_core::{
    ViewState,
    volume::Volume,
    volume_view::{Quality, VolumeView},
};
use orv_render::{
    opaque_material::OpaqueMaterialImage, opaque_surface::OpaqueSurfaceImage,
    surface_pick::PickScope, volume_source::GpuVolumeSource,
};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Geometry {
    #[default]
    Cells,
    Faces,
}
impl Geometry {
    fn label(self) -> &'static str {
        match self {
            Self::Cells => "Mask cells",
            Self::Faces => "Cell faces",
        }
    }
}
pub(super) enum Image {
    Cells(Arc<OpaqueMaterialImage>),
    Faces(Arc<OpaqueSurfaceImage>),
}
impl Image {
    fn paint(&self, pass: &mut wgpu::RenderPass<'_>) {
        match self {
            Self::Cells(image) => image.paint(pass),
            Self::Faces(image) => image.paint(pass),
        }
    }
    #[cfg(all(test, feature = "gpu-tests"))]
    fn cells(&self) -> &Arc<OpaqueMaterialImage> {
        match self {
            Self::Cells(image) => image,
            Self::Faces(_) => panic!("Expected mask-cell image"),
        }
    }
}
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone)]
pub(super) struct Key {
    selection: RegionSelection,
    geometry: Geometry,
    surface_lighting: bool,
    pick: bool,
    display: ViewState,
    state: VolumeView,
    style: OpticalStyle,
    luminance: f32,
    interactive: [u32; 2],
    refined: [u32; 2],
}
impl Key {
    fn same(&self, other: &Self) -> bool {
        self.selection.same(&other.selection)
            && self.geometry == other.geometry
            && self.surface_lighting == other.surface_lighting
            && self.pick == other.pick
            && self.display == other.display
            && self.state == other.state
            && self.style == other.style
            && self.luminance == other.luminance
            && self.interactive == other.interactive
            && self.refined == other.refined
    }
}
struct Request {
    engine: Arc<Mutex<Option<worker::Engine>>>,
    source: Arc<GpuVolumeSource>,
    format: wgpu::TextureFormat,
    data_cancel: Arc<AtomicBool>,
    scope: Option<Arc<PickScope>>,
    key: Key,
    size: [u32; 2],
    refined: bool,
}
struct Completed {
    key: Key,
    refined: bool,
    image: Arc<Image>,
    scope: Option<Arc<PickScope>>,
}
struct View {
    worker: Worker<Request, Completed>,
    engine: Arc<Mutex<Option<worker::Engine>>>,
    source: Arc<GpuVolumeSource>,
    format: wgpu::TextureFormat,
    data_cancel: Arc<AtomicBool>,
    key: Option<Key>,
    front: Option<Completed>,
    generation: u64,
    pending: Option<(Key, bool)>,
    idle_after: Instant,
    error: Option<String>,
    scope: Option<Arc<PickScope>>,
    probe: Option<picking::Probe>,
}
impl Drop for View {
    fn drop(&mut self) {
        self.data_cancel.store(true, Ordering::Release);
        if let Some(scope) = &self.scope {
            scope.revoke();
        }
    }
}
impl View {
    fn new(
        source: Arc<GpuVolumeSource>,
        format: wgpu::TextureFormat,
        ctx: egui::Context,
    ) -> Result<Self, String> {
        Ok(Self {
            worker: Worker::start(ctx, "orv-region-view", worker::run)?,
            engine: Arc::new(Mutex::new(None)),
            source,
            format,
            data_cancel: Arc::new(AtomicBool::new(false)),
            key: None,
            front: None,
            generation: 0,
            pending: None,
            idle_after: Instant::now(),
            error: None,
            scope: None,
            probe: None,
        })
    }
    fn submit(&mut self, key: Key, refined: bool) {
        let size = if refined {
            key.refined
        } else {
            key.interactive
        };
        self.pending = Some((key.clone(), refined));
        self.generation = self.worker.submit(Request {
            engine: self.engine.clone(),
            source: self.source.clone(),
            format: self.format,
            data_cancel: self.data_cancel.clone(),
            scope: self.scope.clone(),
            key,
            size,
            refined,
        });
    }
    fn current_front(&self, key: &Key) -> bool {
        self.front.as_ref().is_some_and(|front| {
            front.key.same(key)
                && (!key.pick
                    || match (&self.scope, &front.scope) {
                        (Some(current), Some(origin)) => {
                            current.is_active() && Arc::ptr_eq(current, origin)
                        }
                        _ => false,
                    })
        })
    }
    fn update(&mut self, key: Key, interacting: bool, ctx: &egui::Context) {
        let now = Instant::now();
        if self.key.as_ref().is_none_or(|old| !old.same(&key)) {
            if let Some(scope) = self.scope.take() {
                scope.revoke();
            }
            if let Some(probe) = &mut self.probe {
                probe.clear();
            }
            if !key.pick {
                self.probe = None;
            }
            if key.pick {
                match PickScope::new() {
                    Ok(scope) => self.scope = Some(scope),
                    Err(error) => {
                        self.data_cancel.store(true, Ordering::Release);
                        self.worker.clear();
                        self.pending = None;
                        self.front = None;
                        self.key = Some(key);
                        self.error = Some(error.into());
                        return;
                    }
                }
            }
            if self.data_cancel.load(Ordering::Acquire)
                || self.key.as_ref().is_some_and(|old| {
                    old.geometry != key.geometry
                        || if key.geometry == Geometry::Faces {
                            !old.selection.same(&key.selection)
                        } else {
                            !old.selection.same_data(&key.selection)
                        }
                })
            {
                self.data_cancel.store(true, Ordering::Release);
                self.data_cancel = Arc::new(AtomicBool::new(false));
            }
            if self.front.as_ref().is_some_and(|old| {
                !old.key.selection.same(&key.selection) || old.key.geometry != key.geometry
            }) {
                self.front = None;
            }
            self.error = None;
            self.idle_after = now + Duration::from_millis(180);
            self.key = Some(key.clone());
            // A running preview may finish for this exact source/map/ID. Keep
            // only the newest desired camera, rather than starving every frame
            // when input arrives faster than one checked GPU computation.
            let finish_preview = self.pending.as_ref().is_some_and(|(running, refined)| {
                !refined
                    && running.selection.same(&key.selection)
                    && running.geometry == key.geometry
                    && running.pick == key.pick
            });
            if !finish_preview {
                self.submit(key.clone(), false);
            }
        }
        if let Some((generation, result)) = self.worker.take()
            && generation == self.generation
        {
            let pending = self.pending.take();
            match result {
                Ok(frame)
                    if frame.key.selection.same(&key.selection)
                        && frame.key.geometry == key.geometry =>
                {
                    self.front = Some(frame)
                }
                Ok(_) => (),
                Err(error) if pending.is_some_and(|(v, _)| v.same(&key)) => {
                    self.error = Some(error);
                }
                Err(_) => (),
            }
        }
        if self.pending.is_none() && self.error.is_none() && !self.current_front(&key) {
            self.submit(key.clone(), false);
        }
        if interacting {
            self.idle_after = now + Duration::from_millis(180);
        }
        let refine = key.state.quality == Quality::Refined
            && self.error.is_none()
            && self.pending.is_none()
            && self
                .front
                .as_ref()
                .is_some_and(|v| v.key.same(&key) && !v.refined);
        if refine {
            if now >= self.idle_after {
                self.submit(key, true);
            } else {
                ctx.request_repaint_after(self.idle_after - now);
            }
        }
    }
    fn suspend(&mut self) {
        if let Some(scope) = self.scope.take() {
            scope.revoke();
        }
        self.probe = None;
        if self.key.is_some() || self.pending.is_some() || self.front.is_some() {
            self.data_cancel.store(true, Ordering::Release);
            self.worker.clear();
            self.key = None;
            self.pending = None;
            self.front = None;
            self.error = None;
        }
    }
}
struct Paint {
    image: Arc<Image>,
}
impl egui_wgpu::CallbackTrait for Paint {
    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        _resources: &egui_wgpu::CallbackResources,
    ) {
        self.image.paint(pass);
    }
}
pub(super) struct Scene {
    pub selection: RegionSelection,
    pub display: ViewState,
    pub state: VolumeView,
    pub style: OpticalStyle,
}
#[derive(Default)]
pub(super) struct Regions {
    settings: controls::Settings,
    source: Option<(Arc<GpuVolumeSource>, wgpu::TextureFormat)>,
    view: Option<View>,
    error: Option<String>,
    picked_point: Option<[f64; 3]>,
    link_epoch: Option<u64>,
}
impl Regions {
    pub fn suspend(&mut self) {
        self.picked_point = None;
        if let Some(view) = &mut self.view {
            view.suspend();
        }
    }
    pub fn select_manual(&mut self) {
        self.settings.select_manual();
    }
    pub fn bind(&mut self, source: Arc<GpuVolumeSource>, format: wgpu::TextureFormat) {
        self.view = None;
        self.source = Some((source, format));
    }
    pub fn set_link_epoch(&mut self, epoch: u64) {
        if self.link_epoch != Some(epoch) {
            self.link_epoch = Some(epoch);
            self.picked_point = None;
            if let Some(probe) = self.view.as_mut().and_then(|view| view.probe.as_mut()) {
                probe.clear();
            }
        }
    }
    pub fn take_point(&mut self) -> Option<[f64; 3]> {
        self.picked_point.take()
    }
    pub fn enabled(&self) -> bool {
        self.settings.enabled
    }
    pub fn disable(&mut self) {
        self.settings.enabled = false;
        self.picked_point = None;
        self.view = None;
        self.error = None;
    }
    pub fn controls(
        &mut self,
        ui: &mut egui::Ui,
        labels: &mut LabelUi,
        selected: usize,
        source: &Arc<Volume>,
        material: &mut bool,
    ) -> Option<RegionSelection> {
        self.settings.ui(ui, labels, selected, source, material);
        self.selection(labels, selected, source)
    }
    pub fn selection(
        &mut self,
        labels: &LabelUi,
        selected: usize,
        source: &Arc<Volume>,
    ) -> Option<RegionSelection> {
        let (selection, waiting) = self
            .settings
            .current(labels, &labels.region_maps(selected, source));
        if selection.is_none() {
            if waiting {
                if let Some(view) = &mut self.view {
                    view.suspend();
                }
            } else {
                self.view = None;
            }
        }
        selection
    }
    pub fn displayed(&self) -> Option<(ViewState, VolumeView, [u32; 2])> {
        let front = self.view.as_ref()?.front.as_ref()?;
        Some((
            front.key.display.clone(),
            front.key.state.clone(),
            if front.refined {
                front.key.refined
            } else {
                front.key.interactive
            },
        ))
    }
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        scene: Scene,
        interacting: bool,
        response: &egui::Response,
    ) -> bool {
        self.picked_point = None;
        let pixels = rect.size() * ui.ctx().pixels_per_point();
        let mut display = scene.display;
        display.invert = false;
        let key = Key {
            selection: scene.selection,
            geometry: self.settings.geometry,
            surface_lighting: self.settings.surface_lighting,
            pick: self.settings.pick && self.settings.geometry == Geometry::Faces,
            display,
            state: scene.state,
            style: scene.style,
            luminance: self.settings.luminance,
            interactive: super::callback::resolution(pixels, 512.),
            refined: super::callback::resolution(pixels, 2048.),
        };
        if self.view.is_none()
            && self.error.is_none()
            && let Some((source, format)) = &self.source
        {
            match View::new(source.clone(), *format, ui.ctx().clone()) {
                Ok(view) => self.view = Some(view),
                Err(error) => self.error = Some(error),
            }
        }
        let mut painted = false;
        if let Some(view) = &mut self.view {
            view.update(key.clone(), interacting, ui.ctx());
            if let Some(front) = &view.front {
                ui.painter_at(rect)
                    .add(egui_wgpu::Callback::new_paint_callback(
                        rect,
                        Paint {
                            image: front.image.clone(),
                        },
                    ));
                painted = true;
            }
            if key.pick {
                if view.probe.is_none() && view.error.is_none() {
                    match picking::Probe::new(ui.ctx().clone()) {
                        Ok(probe) => view.probe = Some(probe),
                        Err(error) => view.error = Some(error),
                    }
                }
                if let Some(probe) = &mut view.probe {
                    self.picked_point = probe.show(
                        ui,
                        response,
                        view.front.as_ref(),
                        &key,
                        &view.source,
                        interacting,
                    );
                }
            }
            if let Some(error) = &view.error {
                let status = if view.front.is_some() {
                    "Region view unchanged"
                } else {
                    "Region unavailable; source shown"
                };
                let retry = ui
                    .horizontal_wrapped(|ui| {
                        ui.label(format!("{status}: {error}"));
                        ui.button("Retry region view").clicked()
                    })
                    .inner;
                if retry {
                    view.key = None;
                    view.error = None;
                }
            } else {
                let status = if view.pending.is_some() {
                    if view.front.is_some() {
                        "Updating view"
                    } else {
                        "Preparing region · source shown until ready"
                    }
                } else if view.front.as_ref().is_some_and(|v| v.refined) {
                    "Refined"
                } else {
                    "Preview"
                };
                if rect.width() < 250. {
                    let status = if view.pending.is_some() {
                        if view.front.is_some() {
                            "updating"
                        } else {
                            "preparing"
                        }
                    } else if view.front.as_ref().is_some_and(|v| v.refined) {
                        "refined"
                    } else {
                        "preview"
                    };
                    ui.small(format!("Region {} · {status}", key.selection.number));
                } else {
                    ui.small(format!(
                        "Region {} · local · {status}",
                        key.selection.number
                    ));
                }
            }
        }
        if let Some(error) = &self.error {
            let retry = ui
                .horizontal_wrapped(|ui| {
                    ui.label(error);
                    ui.button("Retry region view").clicked()
                })
                .inner;
            if retry {
                self.error = None;
            }
        }
        painted
    }
}
