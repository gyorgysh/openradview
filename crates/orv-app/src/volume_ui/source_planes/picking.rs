// SPDX-License-Identifier: GPL-3.0-or-later
//! Exact live-view gates for read-only picks in the mixed source-plane scene.
use super::*;
use orv_render::{
    section_volume::pick::{SectionPickFrame, SectionPickState, SectionProbe},
    surface_pick::SurfaceProbeKind,
};
struct Job {
    engine: Arc<Mutex<Option<Engine>>>,
    frame: SectionPickFrame,
    current: SectionPickState,
    pixel: [u32; 2],
}
struct Pending {
    generation: u64,
    pixel: [u32; 2],
    viewport: (egui::Id, egui::LayerId, egui::Rect, egui::Rect),
}
pub(super) struct Probe {
    worker: Worker<Job, SectionProbe>,
    pending: Option<Pending>,
    notice: Option<String>,
}
impl Probe {
    pub fn new(ctx: egui::Context) -> Result<Self, String> {
        Ok(Self {
            worker: Worker::start(ctx, "orv-section-probe", run)?,
            pending: None,
            notice: None,
        })
    }
    pub fn clear(&mut self) {
        self.worker.clear();
        self.pending = None;
        self.notice = None;
    }
    pub fn show(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        live: Option<(SectionPickFrame, SectionPickState)>,
        engine: Arc<Mutex<Option<Engine>>>,
        interacting: bool,
    ) -> Option<[f64; 3]> {
        let rect = response.rect;
        let viewport = (response.id, response.layer_id, rect, ui.clip_rect());
        if interacting
            || response.double_clicked()
            || !ui.is_enabled()
            || !response.enabled()
            || egui::Popup::is_any_open(ui.ctx())
            || !ui.input(|input| input.focused && !input.key_pressed(egui::Key::Escape))
            || !ui
                .ctx()
                .memory(|memory| memory.allows_interaction(response.layer_id))
        {
            self.clear();
            return None;
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|p| p.viewport != viewport)
        {
            self.clear();
        }
        let mut selected = None;
        if let Some((generation, result)) = self.worker.take()
            && let Some(pending) = self.pending.take()
            && generation == pending.generation
            && pending.viewport == viewport
            && let Some((frame, current)) = &live
        {
            match result {
                Ok(probe) if probe.is_current(current, pending.pixel) => {
                    self.notice = Some(match probe.kind() {
                        SurfaceProbeKind::Hit => {
                            if let Some(hit) = probe.hit() {
                                let cell = hit.face.cell();
                                let point = frame
                                    .image()
                                    .source()
                                    .volume()
                                    .voxel_to_world(cell.map(f64::from));
                                selected = Some(point);
                                format!(
                                    "Source cell {}, {}, {} · L {:.2} P {:.2} S {:.2} mm",
                                    cell[0], cell[1], cell[2], point[0], point[1], point[2]
                                )
                            } else {
                                "No checked source cell".into()
                            }
                        }
                        SurfaceProbeKind::Empty => "No known cell face at this point".into(),
                        SurfaceProbeKind::Obscured => {
                            "Cell face covered by a source plane or foreground source".into()
                        }
                        SurfaceProbeKind::Ambiguous => {
                            "Cell-face contact is ambiguous; choose another point".into()
                        }
                    });
                }
                Err(error) if current.scope.is_active() => self.notice = Some(error),
                _ => (),
            }
        }
        if response.hovered() && live.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        if response.clicked_by(egui::PointerButton::Primary)
            && !response.double_clicked()
            && !crate::viewport::navigation_override(ui, response)
            && ui.input(|input| input.modifiers == egui::Modifiers::NONE)
        {
            selected = None;
            if let Some((frame, current)) = live
                && let Some(pixel) = response.interact_pointer_pos().and_then(|point| {
                    super::super::regions::picking::pixel_at(rect, point, current.scene.size)
                })
            {
                let generation = self.worker.submit(Job {
                    engine,
                    frame,
                    current,
                    pixel,
                });
                self.pending = Some(Pending {
                    generation,
                    pixel,
                    viewport,
                });
                self.notice = Some("Checking the selected cell face…".into());
            } else {
                self.notice = Some("Wait for the current source-plane view before picking".into());
            }
        }
        if let Some(notice) = &self.notice {
            let painter = ui.painter_at(rect);
            let text = painter.layout(
                notice.clone(),
                egui::FontId::monospace(10.),
                crate::theme::PAPER,
                (rect.width() - 12.).max(16.),
            );
            let origin = rect.left_bottom() + egui::vec2(6., -6. - text.size().y);
            painter.rect_filled(
                egui::Rect::from_min_size(
                    origin - egui::vec2(3., 2.),
                    text.size() + egui::vec2(6., 4.),
                ),
                2.,
                egui::Color32::BLACK,
            );
            painter.galley(origin, text, crate::theme::PAPER);
        }
        selected
    }
}
pub(super) fn current(
    front: &Complete,
    scene: &Scene,
) -> Option<(SectionPickFrame, SectionPickState)> {
    if !(scene.pick || scene.erase) || !front.scene.same(scene) {
        return None;
    }
    let selection = scene.selection.as_ref()?;
    let scope = front.scope.as_ref()?;
    if !scope.is_active() || *front.image.scene().material != scene.style.material {
        return None;
    }
    let frame = SectionPickFrame::new(front.image.clone(), scope.clone());
    // Reuse only the prepared interface and matching material owner. Every live
    // CPU request field is re-applied; a retained frame cannot prove it is current.
    let mut desired = SectionScene {
        planes: scene.planes.clone(),
        surface: front.image.scene().surface.clone(),
        navigation: scene.display.clone(),
        volume: scene.state.clone(),
        material: front.image.scene().material.clone(),
        lighting: scene.style.lighting,
        size: scene.size,
    };
    let surface = desired.surface.as_mut()?;
    surface.luminance = scene.luminance;
    surface.lighting = if scene.surface_lighting {
        Lighting::HEADLIGHT
    } else {
        Lighting::UNLIT
    };
    let current = SectionPickState {
        scope: scope.clone(),
        map: selection.map.clone(),
        number: selection.number,
        scene: desired,
    };
    current.matches(&frame).then_some((frame, current))
}
fn run(input: Job, cancel: &dyn Fn() -> bool) -> Result<SectionProbe, String> {
    let cancelled = || cancel() || !input.current.scope.is_active();
    if cancelled() {
        return Err("Cancelled".into());
    }
    let mut engine = input
        .engine
        .lock()
        .map_err(|_| "Section probe unavailable; reopen the view")?;
    if cancelled() {
        return Err("Cancelled".into());
    }
    engine
        .as_mut()
        .ok_or("Section probe unavailable")?
        .renderer
        .probe(&input.frame, &input.current, input.pixel, cancelled)
        .map_err(str::to_owned)
}

#[cfg(all(test, feature = "gpu-tests"))]
mod tests;
