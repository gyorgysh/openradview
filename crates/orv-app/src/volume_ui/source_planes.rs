// SPDX-License-Identifier: GPL-3.0-or-later
//! Local completed-plane actors and checked read-only region probes.
mod picking;
use super::{material::OpticalStyle, regions::surface::Faces};
use crate::{jobs::Worker, labels::RegionSelection};
use eframe::{
    egui,
    egui_wgpu::{self, wgpu},
};
use orv_core::{
    ViewState, WindowLevel,
    volume_view::{Quality, VolumeView},
};
use orv_render::{
    material_volume::Lighting,
    reslice::GpuReformatImage,
    section_volume::{SectionImage, SectionPlane, SectionRenderer, SectionScene, SectionSurface},
    surface_pick::PickScope,
    volume_source::GpuVolumeSource,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Copy)]
pub(crate) struct LinkedPlanes<'a> {
    pub images: &'a [Option<Arc<GpuReformatImage>>; 3],
    pub window: WindowLevel,
    pub invert: bool,
    pub current: bool,
}
pub(super) struct Settings {
    pub enabled: bool,
    visible: [bool; 3],
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            visible: [true; 3],
        }
    }
}
impl Settings {
    pub fn active(&self) -> bool {
        self.enabled && self.visible.into_iter().any(|v| v)
    }
    pub fn ui(&mut self, ui: &mut egui::Ui) -> bool {
        let was_active = self.active();
        ui.checkbox(&mut self.enabled, "Source planes in 3D");
        if self.enabled {
            ui.horizontal_wrapped(|ui| {
                for (visible, label) in self
                    .visible
                    .iter_mut()
                    .zip(["Axial", "Coronal", "Sagittal"])
                {
                    ui.checkbox(visible, label);
                }
            });
            ui.small("Planes follow the completed MPR images and their contrast. Regions use cell faces. Visible planes and foreground source can block region picks. Choosing Volume or MIP hides the planes.");
        }
        !was_active && self.active()
    }
    pub fn planes(
        &self,
        input: Option<LinkedPlanes<'_>>,
    ) -> Result<[Option<SectionPlane>; 3], &'static str> {
        let input = input.ok_or("Source planes are unavailable for these images")?;
        let mut planes = std::array::from_fn(|_| None);
        for (i, plane) in planes.iter_mut().enumerate() {
            if !self.visible[i] {
                continue;
            }
            let image = input.images[i]
                .as_ref()
                .ok_or("Source planes are unavailable for these images")?;
            if image.grid().slab().is_some() {
                return Err("Choose Thin plane to show source planes in 3D");
            }
            *plane = Some(SectionPlane {
                image: image.clone(),
                window: input.window,
                invert: input.invert,
            });
        }
        Ok(planes)
    }
}
#[derive(Clone)]
pub(super) struct Scene {
    pub pick: bool,
    pub planes: [Option<SectionPlane>; 3],
    pub selection: Option<RegionSelection>,
    pub display: ViewState,
    pub state: VolumeView,
    pub style: OpticalStyle,
    pub luminance: f32,
    pub surface_lighting: bool,
    pub size: [u32; 2],
}
impl Scene {
    fn same_data(&self, other: &Self) -> bool {
        match (&self.selection, &other.selection) {
            (None, None) => true,
            (Some(a), Some(b)) => a.same(b),
            _ => false,
        }
    }
    fn same(&self, other: &Self) -> bool {
        self.pick == other.pick
            && self.same_data(other)
            && self
                .planes
                .iter()
                .zip(&other.planes)
                .all(|(a, b)| match (a, b) {
                    (None, None) => true,
                    (Some(a), Some(b)) => {
                        Arc::ptr_eq(&a.image, &b.image)
                            && a.window == b.window
                            && a.invert == b.invert
                    }
                    _ => false,
                })
            && self.display == other.display
            && self.state == other.state
            && self.style == other.style
            && self.luminance == other.luminance
            && self.surface_lighting == other.surface_lighting
            && self.size == other.size
    }
}
struct Engine {
    source: Arc<GpuVolumeSource>,
    renderer: SectionRenderer,
    faces: Option<Faces>,
    material: Option<Arc<orv_core::volume_material::VolumeMaterial>>,
}
impl Engine {
    fn new(source: Arc<GpuVolumeSource>, format: wgpu::TextureFormat) -> Result<Self, String> {
        Ok(Self {
            renderer: SectionRenderer::new(source.clone(), format).map_err(str::to_owned)?,
            source,
            faces: None,
            material: None,
        })
    }
    fn render(
        &mut self,
        scene: &Scene,
        data_cancel: &dyn Fn() -> bool,
        cancel: &dyn Fn() -> bool,
    ) -> Result<Arc<SectionImage>, String> {
        if cancel() {
            return Err("Cancelled".into());
        }
        let surface = if let Some(selection) = &scene.selection {
            if self.faces.is_none() {
                self.faces = Some(Faces::new(self.source.clone())?);
            }
            let image = self
                .faces
                .as_mut()
                .ok_or("Cell faces unavailable")?
                .project(
                    selection,
                    &scene.display,
                    &scene.state,
                    scene.size,
                    data_cancel,
                    cancel,
                )?;
            Some(SectionSurface {
                image,
                luminance: scene.luminance,
                lighting: if scene.surface_lighting {
                    Lighting::HEADLIGHT
                } else {
                    Lighting::UNLIT
                },
            })
        } else {
            self.faces = None;
            None
        };
        if self
            .material
            .as_ref()
            .is_none_or(|m| **m != scene.style.material)
        {
            self.material = Some(Arc::new(scene.style.material.clone()));
        }
        let input = SectionScene {
            planes: scene.planes.clone(),
            surface,
            navigation: scene.display.clone(),
            volume: scene.state.clone(),
            material: self
                .material
                .clone()
                .ok_or("Source plane material unavailable")?,
            lighting: scene.style.lighting,
            size: scene.size,
        };
        let mut complete = None;
        for y in (0..scene.size[1]).step_by(16) {
            complete = self
                .renderer
                .render_rows(&input, 0, y..(y + 16).min(scene.size[1]), cancel)
                .map_err(str::to_owned)?;
            if complete.is_none() {
                self.source.wait_submitted(cancel).map_err(str::to_owned)?;
            }
        }
        complete.ok_or_else(|| "Source planes did not complete".into())
    }
}
struct Request {
    engine: Arc<Mutex<Option<Engine>>>,
    source: Arc<GpuVolumeSource>,
    format: wgpu::TextureFormat,
    scene: Scene,
    data_cancel: Arc<AtomicBool>,
    scope: Option<Arc<PickScope>>,
}
struct Complete {
    scope: Option<Arc<PickScope>>,
    scene: Scene,
    image: Arc<SectionImage>,
}
fn run(input: Request, cancel: &dyn Fn() -> bool) -> Result<Complete, String> {
    if cancel() {
        return Err("Cancelled".into());
    }
    let mut engine = input
        .engine
        .lock()
        .map_err(|_| "Source plane renderer unavailable")?;
    if engine.is_none() {
        *engine = Some(Engine::new(input.source, input.format)?);
    }
    let engine = engine.as_mut().ok_or("Source plane renderer unavailable")?;
    let data_cancel = || input.data_cancel.load(Ordering::Acquire);
    match engine.render(&input.scene, &data_cancel, cancel) {
        Ok(image) => Ok(Complete {
            scope: input.scope,
            scene: input.scene,
            image,
        }),
        Err(error) => {
            let _ = engine.renderer.cancel(0);
            Err(error)
        }
    }
}
pub(super) struct View {
    scope: Option<Arc<PickScope>>,
    probe: Option<picking::Probe>,
    picked_point: Option<[f64; 3]>,
    link_epoch: Option<u64>,
    viewport: Option<(egui::Id, egui::LayerId, egui::Rect, egui::Rect)>,
    worker: Worker<Request, Complete>,
    engine: Arc<Mutex<Option<Engine>>>,
    source: Arc<GpuVolumeSource>,
    format: wgpu::TextureFormat,
    data_cancel: Arc<AtomicBool>,
    desired: Option<Scene>,
    pending: Option<Scene>,
    front: Option<Complete>,
    generation: u64,
    error: Option<String>,
}
impl Drop for View {
    fn drop(&mut self) {
        self.clear_probe();
        self.data_cancel.store(true, Ordering::Release);
    }
}
impl View {
    pub fn new(
        source: Arc<GpuVolumeSource>,
        format: wgpu::TextureFormat,
        ctx: egui::Context,
    ) -> Result<Self, String> {
        Ok(Self {
            scope: None,
            probe: None,
            picked_point: None,
            link_epoch: None,
            viewport: None,
            worker: Worker::start(ctx, "orv-source-planes", run)?,
            engine: Arc::new(Mutex::new(None)),
            source,
            format,
            data_cancel: Arc::new(AtomicBool::new(false)),
            desired: None,
            pending: None,
            front: None,
            generation: 0,
            error: None,
        })
    }
    fn submit(&mut self, scene: Scene) {
        self.pending = Some(scene.clone());
        self.generation = self.worker.submit(Request {
            engine: self.engine.clone(),
            source: self.source.clone(),
            format: self.format,
            scene,
            data_cancel: self.data_cancel.clone(),
            scope: self.scope.clone(),
        });
    }
    fn clear_probe(&mut self) {
        if let Some(scope) = self.scope.take() {
            scope.revoke();
        }
        if let Some(probe) = &mut self.probe {
            probe.clear();
        }
        self.picked_point = None;
    }
    pub fn set_link_epoch(&mut self, epoch: u64) {
        if self.link_epoch != Some(epoch) {
            self.link_epoch = Some(epoch);
            if let Some(probe) = &mut self.probe {
                probe.clear();
            }
            self.picked_point = None;
        }
    }
    pub fn take_point(&mut self) -> Option<[f64; 3]> {
        self.picked_point.take()
    }
    fn current_front(&self, scene: &Scene) -> bool {
        self.front.as_ref().is_some_and(|front| {
            front.scene.same(scene)
                && (!scene.pick
                    || matches!((&self.scope,&front.scope),
                (Some(a),Some(b)) if a.is_active() && Arc::ptr_eq(a,b)))
        })
    }
    fn update(&mut self, scene: &Scene) {
        if self.desired.as_ref().is_none_or(|old| !old.same(scene)) {
            self.clear_probe();
            if scene.pick {
                match PickScope::new() {
                    Ok(scope) => self.scope = Some(scope),
                    Err(error) => {
                        self.suspend();
                        self.desired = Some(scene.clone());
                        self.error = Some(error.into());
                        return;
                    }
                }
            } else {
                self.probe = None;
            }

            if self.data_cancel.load(Ordering::Acquire)
                || self
                    .desired
                    .as_ref()
                    .is_some_and(|old| !old.same_data(scene))
            {
                self.data_cancel.store(true, Ordering::Release);
                self.data_cancel = Arc::new(AtomicBool::new(false));
                self.worker.clear();
                self.pending = None;
                self.front = None;
            }
            self.desired = Some(scene.clone());
            self.error = None;
        }
        if let Some((generation, result)) = self.worker.take()
            && generation == self.generation
        {
            let pending = self.pending.take();
            match result {
                Ok(frame) if frame.scene.same_data(scene) => self.front = Some(frame),
                Ok(_) => (),
                Err(error) if pending.is_some_and(|p| p.same(scene)) => self.error = Some(error),
                Err(_) => (),
            }
        }
        // Finish one coherent image before chasing new camera/plane inputs.
        // Input cannot starve completion, and old data never gains a new identity.
        if self.pending.is_none() && self.error.is_none() && !self.current_front(scene) {
            self.submit(scene.clone());
        }
    }
    pub fn suspend(&mut self) {
        self.clear_probe();
        self.data_cancel.store(true, Ordering::Release);
        self.worker.clear();
        self.desired = None;
        self.pending = None;
        self.front = None;
        self.error = None;
    }
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        response: &egui::Response,
        scene: Scene,
        interacting: bool,
        source_current: bool,
    ) -> Option<(ViewState, VolumeView, [u32; 2])> {
        let rect = response.rect;
        let viewport = (response.id, response.layer_id, rect, ui.clip_rect());
        if self.viewport != Some(viewport) {
            self.clear_probe();
            self.desired = None;
            self.viewport = Some(viewport);
        }
        self.picked_point = None;
        self.update(&scene);
        if scene.pick && self.probe.is_none() {
            match picking::Probe::new(ui.ctx().clone()) {
                Ok(probe) => self.probe = Some(probe),
                Err(error) => {
                    self.error = Some(error);
                }
            }
        }
        let current = self.error.is_none() && self.current_front(&scene);
        if let Some(error) = &self.error {
            message(ui, rect, error);
        } else if let Some(front) = &self.front {
            ui.painter_at(rect)
                .add(egui_wgpu::Callback::new_paint_callback(
                    rect,
                    Paint {
                        image: front.image.clone(),
                    },
                ));
        } else {
            message(ui, rect, "Preparing source planes…");
        }
        if self.error.is_some() {
            if ui.button("Retry planes").clicked() {
                self.error = None;
                self.desired = None;
            }
        } else {
            ui.small(if self.pending.is_some() {
                "Planes · updating"
            } else if scene.state.quality == Quality::Refined {
                "Planes · full detail"
            } else {
                "Planes · preview"
            });
        }
        let live = if current && source_current {
            self.front
                .as_ref()
                .and_then(|front| picking::current(front, &scene))
        } else {
            None
        };
        if let Some(probe) = &mut self.probe {
            self.picked_point = probe.show(
                ui,
                response,
                live,
                self.engine.clone(),
                interacting || !source_current,
            );
        }
        if current {
            self.front.as_ref().map(|front| {
                (
                    front.scene.display.clone(),
                    front.scene.state.clone(),
                    front.scene.size,
                )
            })
        } else {
            None
        }
    }
}
pub(super) fn message(ui: &egui::Ui, rect: egui::Rect, message: &str) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0., egui::Color32::BLACK);
    let text = painter.layout(
        message.into(),
        egui::FontId::proportional(12.),
        crate::theme::PAPER,
        (rect.width() - 16.).max(20.),
    );
    painter.galley(rect.center() - text.size() * 0.5, text, crate::theme::PAPER);
}
struct Paint {
    image: Arc<SectionImage>,
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

#[cfg(all(test, feature = "gpu-tests"))]
mod tests;
