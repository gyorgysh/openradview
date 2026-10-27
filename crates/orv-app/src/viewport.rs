// SPDX-License-Identifier: GPL-3.0-or-later
use eframe::{
    egui,
    egui_wgpu::{self, wgpu},
};
use orv_core::{ViewState, study::Frame};
use orv_render::slice::SliceRenderer;

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
    pub error: Option<&'static str>,
}

pub struct SliceCallback {
    pub slot: usize,
    pub repaint: egui::Context,
    pub image: Arc<Frame>,
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
            let engine = state
                .engine
                .get_or_insert_with(|| SliceRenderer::new(device, self.format));
            state.error = None;
            if !state
                .current
                .as_ref()
                .is_some_and(|f| Arc::ptr_eq(f, &self.image))
            {
                if let Err(e) = engine.upload(device, queue, &self.image) {
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
            if state.error.is_some() && previous != state.error {
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
    if response.hovered() {
        let (wheel, zoom, modifiers, scrolling) = ui.input_mut(|i| {
            let input = (
                i.smooth_scroll_delta,
                i.zoom_delta(),
                i.modifiers,
                i.is_scrolling(),
            );
            i.smooth_scroll_delta = egui::Vec2::ZERO;
            input
        });
        let zoom_gesture = (zoom - 1.0).abs() > f32::EPSILON || modifiers.command || modifiers.ctrl;
        let suppression_id = response.id.with("zoom-owns-scroll-sequence");
        let suppress_browse = ui.ctx().data_mut(|d| {
            let mut suppressed = d.get_temp::<bool>(suppression_id).unwrap_or(false);
            if zoom_gesture {
                suppressed = true;
            } else if !scrolling {
                suppressed = false;
            }
            d.insert_temp(suppression_id, suppressed);
            suppressed
        });
        // Pinch and modifier-wheel may arrive in the same batch. Apply zoom
        // once, and never turn a zoom gesture into a slice change as well.
        if (zoom - 1.0).abs() > f32::EPSILON {
            view.zoom = (view.zoom * zoom).clamp(0.05, 50.0);
        } else if modifiers.command || modifiers.ctrl || tool == Tool::Zoom {
            view.zoom = (view.zoom * (wheel.y * 0.004).exp()).clamp(0.05, 50.0);
        } else if matches!(tool, Tool::Browse | Tool::Crosshair) {
            // Egui smooths wheel input into later frames. A pinch must retain
            // ownership until that scroll sequence ends, including its tail.
            if !suppress_browse {
                advance_slice(view, scroll, -wheel.y, slice_count);
            }
        } else {
            pan(view, rect, wheel);
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
    if response.double_clicked() {
        view.zoom = 1.0;
        view.pan = [0.0, 0.0];
    }
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
    ui.input(|i| i.key_down(egui::Key::Space))
        || response.dragged_by(egui::PointerButton::Secondary)
        || response.dragged_by(egui::PointerButton::Middle)
}

fn pan(view: &mut ViewState, rect: egui::Rect, delta: egui::Vec2) {
    view.pan[0] = (view.pan[0] + delta.x / rect.width().max(1.0)).clamp(-10.0, 10.0);
    view.pan[1] = (view.pan[1] + delta.y / rect.height().max(1.0)).clamp(-10.0, 10.0);
}

fn advance_slice(view: &mut ViewState, scroll: &mut f32, delta: f32, count: usize) {
    *scroll += delta;
    let steps = (*scroll / 25.0).trunc() as i32;
    if steps != 0 {
        view.slice =
            (view.slice as i64 + steps as i64).clamp(0, count.saturating_sub(1) as i64) as u32;
        *scroll -= steps as f32 * 25.0;
    }
}

impl Tool {
    pub fn measures(self) -> bool {
        matches!(self, Self::Distance | Self::Angle | Self::Roi)
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
    if let Some(render) = frame.wgpu_render_state()
        && let Some(renderers) = render
            .renderer
            .write()
            .callback_resources
            .get_mut::<ViewportRenderers>()
    {
        renderers.slots.remove(&slot);
    }
}

pub fn draw(
    ui: &mut egui::Ui,
    frame: &eframe::Frame,
    rect: egui::Rect,
    image: Arc<Frame>,
    view: &ViewState,
    slot: usize,
    label: &str,
) {
    let painter = ui.painter_at(rect);
    if let Some(render) = frame.wgpu_render_state() {
        painter.add(egui_wgpu::Callback::new_paint_callback(
            rect,
            SliceCallback {
                slot,
                repaint: ui.ctx().clone(),
                image,
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
