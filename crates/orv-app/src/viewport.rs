// SPDX-License-Identifier: GPL-3.0-or-later
use eframe::{
    egui,
    egui_wgpu::{self, wgpu},
};
use orv_core::{ViewState, study::Frame};
use orv_render::{reslice::GpuReformatImage, slice::SliceRenderer, volume_source::GpuVolumeSource};

#[cfg(all(test, feature = "gpu-tests"))]
#[path = "gpu_viewport_tests.rs"]
mod gpu_tests;
#[cfg(test)]
#[path = "viewport_tests.rs"]
mod tests;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Default)]
pub struct ViewportRenderers {
    slots: BTreeMap<usize, ViewportRenderer>,
}

#[derive(Default)]
struct ViewportRenderer {
    engine: Option<SliceRenderer>,
    current: Option<Arc<Frame>>,
    gpu_source: Option<Arc<GpuVolumeSource>>,
    pub error: Option<&'static str>,
}

pub struct SliceCallback {
    pub slot: usize,
    pub repaint: egui::Context,
    pub image: Arc<Frame>,
    pub gpu_image: Option<Arc<GpuReformatImage>>,
    pub view: ViewState,
    pub aspect: f32,
    pub format: wgpu::TextureFormat,
}
impl egui_wgpu::CallbackTrait for SliceCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if resources.get::<ViewportRenderers>().is_none() {
            resources.insert(ViewportRenderers::default());
        }
        if let Some(renderers) = resources.get_mut::<ViewportRenderers>() {
            let state = renderers.slots.entry(self.slot).or_default();
            let previous = state.error;
            state.error = None;
            if self
                .gpu_image
                .as_ref()
                .is_some_and(|g| !Arc::ptr_eq(g.frame(), &self.image))
            {
                state.error = Some("Completed plane and display image differ");
                if previous != state.error {
                    self.repaint.request_repaint();
                }
                return vec![];
            }
            let source = self.gpu_image.as_ref().map(|g| g.source());
            let same_source = match (&state.gpu_source, source) {
                (None, None) => true,
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                _ => false,
            };
            if state.engine.is_none() || !same_source {
                let prepared = if let Some(image) = &self.gpu_image {
                    SliceRenderer::from_reslice(image.clone(), self.format)
                } else {
                    Ok(SliceRenderer::new(device, self.format))
                };
                match prepared {
                    Ok(engine) => {
                        state.engine = Some(engine);
                        state.gpu_source = source.cloned();
                        state.current = self.gpu_image.as_ref().map(|_| self.image.clone());
                    }
                    Err(error) => {
                        state.error = Some(error);
                        if previous != state.error {
                            self.repaint.request_repaint();
                        }
                        return vec![];
                    }
                }
            }
            let Some(engine) = &mut state.engine else {
                return vec![];
            };
            if !state
                .current
                .as_ref()
                .is_some_and(|f| Arc::ptr_eq(f, &self.image))
            {
                let upload = if let Some(image) = &self.gpu_image {
                    engine.upload_reslice(image.clone())
                } else {
                    engine.upload(device, queue, &self.image)
                };
                if let Err(e) = upload {
                    state.error = Some(e);
                    if previous != state.error {
                        self.repaint.request_repaint();
                    }
                    return vec![];
                }
                state.current = Some(self.image.clone());
            }
            if let Err(e) = engine.set_view(queue, &self.image, &self.view, self.aspect) {
                state.error = Some(e);
            }
            // The UI paints the previous callback state; redraw on both failure
            // and recovery so a resolved error does not remain over the image.
            if previous != state.error {
                self.repaint.request_repaint();
            }
        }
        vec![]
    }
    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        if let Some(state) = resources
            .get::<ViewportRenderers>()
            .and_then(|r| r.slots.get(&self.slot))
            && state.error.is_none()
            && let Some(engine) = &state.engine
        {
            engine.paint(pass);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Tool {
    Window,
    #[default]
    Pan,
    Zoom,
    Browse,
    Crosshair,
    Distance,
    Angle,
    Roi,
    EllipseRoi,
    PolygonRoi,
    EditMarks,
}

pub fn interact(
    ui: &mut egui::Ui,
    response: &egui::Response,
    view: &mut ViewState,
    tool: Tool,
    slice_count: usize,
    scroll: &mut f32,
) {
    let rect = response.rect;
    let owner = zoom_scroll_owner(ui, response, tool == Tool::Zoom);
    if response.hovered() {
        let (wheel, zoom, modifiers) = ui.input_mut(|i| {
            let input = (i.smooth_scroll_delta, i.zoom_delta(), i.modifiers);
            i.smooth_scroll_delta = egui::Vec2::ZERO;
            input
        });
        // Egui smooths wheel input into later frames, even if the pointer
        // moves to another pane before the zoom sequence ends.
        if owner.is_none_or(|owner| owner == response.id) {
            // Pinch and modifier-wheel may arrive in the same batch. Apply
            // zoom once, and never also turn it into a slice change.
            if (zoom - 1.0).abs() > f32::EPSILON {
                view.zoom = (view.zoom * zoom).clamp(0.05, 50.0);
            } else if modifiers.command || modifiers.ctrl || tool == Tool::Zoom {
                view.zoom = (view.zoom * (wheel.y * 0.004).exp()).clamp(0.05, 50.0);
            } else if matches!(tool, Tool::Browse | Tool::Crosshair) && owner.is_none() {
                advance_slice(view, scroll, -wheel.y, slice_count);
            } else if owner.is_none() && !matches!(tool, Tool::Browse | Tool::Crosshair) {
                pan(view, rect, wheel);
            }
        }
    }
    if response.dragged() {
        let delta = ui.input(|i| i.pointer.delta());
        if navigation_override(ui, response) || tool == Tool::Pan {
            pan(view, rect, delta);
        } else if tool == Tool::Browse {
            advance_slice(view, scroll, -delta.y, slice_count);
        } else if tool == Tool::Zoom {
            view.zoom = (view.zoom * (-delta.y * 0.012).exp()).clamp(0.05, 50.0);
        } else if tool == Tool::Window {
            adjust_contrast(&mut view.window, delta);
        }
    }
    if response.double_clicked() && !tool.measures() {
        view.zoom = 1.0;
        view.pan = [0.0, 0.0];
    }
}

/// Share gesture ownership between image and 3D panes in the same viewport.
pub(crate) fn zoom_scroll_owner(
    ui: &egui::Ui,
    response: &egui::Response,
    scroll_zooms: bool,
) -> Option<egui::Id> {
    let id = egui::Id::new("orv-viewport-zoom-owns-scroll-sequence").with(ui.ctx().viewport_id());
    let mut ownership = zoom_scroll_ownership(ui, id);
    if response.hovered()
        && ownership.owner.is_none()
        && ui.input(|input| {
            (input.zoom_delta() - 1.).abs() > f32::EPSILON
                || input.modifiers.command
                || input.modifiers.ctrl
                || (scroll_zooms && input.smooth_scroll_delta != egui::Vec2::ZERO)
        })
    {
        ownership.owner = Some(response.id);
        ui.ctx().data_mut(|data| data.insert_temp(id, ownership));
    }
    ownership.owner
}

#[derive(Clone, Copy, Default)]
struct ZoomScrollOwnership {
    owner: Option<egui::Id>,
    last_wheel: Option<f64>,
    last_zoom: Option<f64>,
    touching: bool,
    observed_pass: Option<u64>,
}

fn zoom_scroll_ownership(ui: &egui::Ui, id: egui::Id) -> ZoomScrollOwnership {
    let pass = ui.ctx().cumulative_pass_nr();
    let mut state = ui
        .ctx()
        .data(|data| data.get_temp::<ZoomScrollOwnership>(id))
        .unwrap_or_default();
    if state.observed_pass != Some(pass) {
        ui.input(|input| {
            for event in &input.events {
                match event {
                    egui::Event::MouseWheel { phase, .. } => {
                        match phase {
                            egui::TouchPhase::Start => {
                                state.owner = None;
                                state.last_zoom = None;
                                state.touching = true;
                            }
                            egui::TouchPhase::Move => {
                                // Egui applies this same timeout to phase-less
                                // wheel input, but a sleeping UI may not observe
                                // the idle frame before the next event arrives.
                                if !state.touching
                                    && state
                                        .last_wheel
                                        .is_some_and(|last| input.time - last > 0.150)
                                {
                                    state.owner = None;
                                }
                            }
                            egui::TouchPhase::End | egui::TouchPhase::Cancel => {
                                state.owner = None;
                                state.last_zoom = None;
                                state.touching = false;
                            }
                        }
                        state.last_wheel = Some(input.time);
                    }
                    egui::Event::Zoom(_) => {
                        // The pinned native adapter omits pinch phases. Keep
                        // consecutive magnification events together using the
                        // same idle gap as phase-less wheel input.
                        if !state.touching
                            && !input.is_scrolling()
                            && state.last_zoom.is_none_or(|last| input.time - last > 0.150)
                        {
                            state.owner = None;
                        }
                        state.last_zoom = Some(input.time);
                    }
                    egui::Event::WindowFocused(false) => {
                        state = ZoomScrollOwnership::default();
                    }
                    _ => {}
                }
            }
            let zooming = state
                .last_zoom
                .is_some_and(|last| input.time - last <= 0.150);
            if !input.is_scrolling() && !zooming {
                state.owner = None;
                state.touching = false;
            }
        });
        state.observed_pass = Some(pass);
        ui.ctx().data_mut(|data| data.insert_temp(id, state));
    }
    state
}

fn adjust_contrast(window: &mut orv_core::WindowLevel, delta: egui::Vec2) {
    use orv_core::VoiFunction;
    if matches!(window.function, VoiFunction::Lookup { .. }) {
        return;
    }
    let minimum = if window.function == VoiFunction::Linear {
        1.
    } else {
        f32::MIN_POSITIVE
    };
    window.width = (window.width * (delta.x * 0.007).exp()).clamp(minimum, 100_000.);
    window.center = (window.center - delta.y * window.width * 0.003).clamp(-1_000_000., 1_000_000.);
}

pub fn navigation_override(ui: &egui::Ui, response: &egui::Response) -> bool {
    (ui.is_enabled()
        && !egui::Popup::is_any_open(ui.ctx())
        && (response.has_focus() || !ui.ctx().egui_wants_keyboard_input())
        && ui.input(|i| i.key_down(egui::Key::Space)))
        || response.dragged_by(egui::PointerButton::Secondary)
        || response.dragged_by(egui::PointerButton::Middle)
}

fn pan(view: &mut ViewState, rect: egui::Rect, delta: egui::Vec2) {
    view.pan[0] = (view.pan[0] + delta.x / rect.width().max(1.0)).clamp(-10.0, 10.0);
    view.pan[1] = (view.pan[1] + delta.y / rect.height().max(1.0)).clamp(-10.0, 10.0);
}

fn advance_slice(view: &mut ViewState, scroll: &mut f32, delta: f32, count: usize) {
    let last = count.saturating_sub(1) as i64;
    let outside = |slice: u32, remainder: f32| {
        (slice == 0 && remainder < 0.) || (i64::from(slice) >= last && remainder > 0.)
    };
    // A slider or keyboard action can have reached an edge since the last
    // wheel event. Do not let an unusable remainder resist a new direction.
    if outside(view.slice, *scroll) {
        *scroll = 0.;
    }
    *scroll += delta;
    let steps = (*scroll / 25.0).trunc() as i32;
    if steps != 0 {
        view.slice = (i64::from(view.slice) + i64::from(steps)).clamp(0, last) as u32;
        *scroll -= steps as f32 * 25.0;
    }
    if outside(view.slice, *scroll) {
        *scroll = 0.;
    }
}

impl Tool {
    pub fn measures(self) -> bool {
        matches!(self, Self::Distance | Self::Angle | Self::EditMarks) || self.roi()
    }
    pub fn roi(self) -> bool {
        matches!(self, Self::Roi | Self::EllipseRoi | Self::PolygonRoi)
    }
}

pub fn clear(frame: &eframe::Frame) {
    if let Some(render) = frame.wgpu_render_state() {
        render
            .renderer
            .write()
            .callback_resources
            .remove::<ViewportRenderers>();
    }
}

pub fn clear_slot(frame: &eframe::Frame, slot: usize) {
    if let Some(render) = frame.wgpu_render_state() {
        remove_slot(&mut render.renderer.write().callback_resources, slot);
    }
}
fn remove_slot(resources: &mut egui_wgpu::CallbackResources, slot: usize) {
    if let Some(renderers) = resources.get_mut::<ViewportRenderers>() {
        renderers.slots.remove(&slot);
    }
}

pub enum SliceImage {
    Cpu(Arc<Frame>),
    Gpu(Arc<GpuReformatImage>),
}
impl From<Arc<Frame>> for SliceImage {
    fn from(frame: Arc<Frame>) -> Self {
        Self::Cpu(frame)
    }
}
impl From<Arc<GpuReformatImage>> for SliceImage {
    fn from(image: Arc<GpuReformatImage>) -> Self {
        Self::Gpu(image)
    }
}
pub fn draw(
    ui: &mut egui::Ui,
    frame: &eframe::Frame,
    rect: egui::Rect,
    image: impl Into<SliceImage>,
    view: &ViewState,
    slot: usize,
    label: &str,
) {
    let painter = ui.painter_at(rect);
    let (image, gpu_image) = match image.into() {
        SliceImage::Cpu(frame) => (frame, None),
        SliceImage::Gpu(image) => (image.frame().clone(), Some(image)),
    };
    if let Some(render) = frame.wgpu_render_state() {
        painter.add(egui_wgpu::Callback::new_paint_callback(
            rect,
            SliceCallback {
                slot,
                repaint: ui.ctx().clone(),
                image,
                gpu_image,
                view: view.clone(),
                aspect: rect.aspect_ratio(),
                format: render.target_format,
            },
        ));
        if let Some(error) = render
            .renderer
            .read()
            .callback_resources
            .get::<ViewportRenderers>()
            .and_then(|r| r.slots.get(&slot))
            .and_then(|s| s.error)
        {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                error,
                egui::FontId::proportional(14.0),
                crate::theme::PAPER,
            );
        }
    }
    painter.rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(1.0, crate::theme::GRAPHITE),
        egui::StrokeKind::Inside,
    );
    if !label.is_empty() {
        painter.text(
            rect.min + egui::Vec2::splat(12.0),
            egui::Align2::LEFT_TOP,
            label,
            egui::FontId::monospace(10.0),
            crate::theme::SILVER,
        );
    }
}
