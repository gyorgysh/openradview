// SPDX-License-Identifier: GPL-3.0-or-later
//! One-click manual corrections over the frozen complete base view.
use super::*;
use orv_core::labels::{BrushAction, BrushShape, Labels, PhysicalBrush};
use orv_render::section_volume::pick::{SectionFaceErase, SectionPickFrame, SectionPickState};
#[cfg(all(test, feature = "gpu-tests"))]
mod tests;

#[derive(Clone)]
pub(super) struct Input {
    pub base: Arc<Labels>,
    pub number: u8,
    pub series: usize,
    pub link_epoch: u64,
}
impl Input {
    fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.base, &other.base)
            && self.number == other.number
            && self.series == other.series
            && self.link_epoch == other.link_epoch
    }
    fn matches(&self, current: &SectionPickState) -> bool {
        matches!(&current.map, crate::labels::RegionMap::Manual(base) if Arc::ptr_eq(base, &self.base))
            && current.number == u16::from(self.number)
    }
    fn brush(&self, radius: f64, current: &SectionPickState) -> PhysicalBrush {
        PhysicalBrush {
            label: self.number,
            action: BrushAction::Erase,
            radius_mm: radius,
            shape: BrushShape::Sphere,
            clip: current.scene.volume.clip.clone(),
            protected_ids: [false; 256],
        }
    }
}
struct Job {
    engine: Arc<Mutex<Option<Engine>>>,
    frame: SectionPickFrame,
    current: SectionPickState,
    pixel: [u32; 2],
    brush: PhysicalBrush,
}
struct Output {
    proposal: SectionFaceErase,
    image: Arc<SectionImage>,
}
struct Pending {
    generation: u64,
    input: Input,
    brush: PhysicalBrush,
    viewport: (egui::Id, egui::LayerId, egui::Rect, egui::Rect),
}
pub(in crate::volume_ui) struct Apply {
    pub proposal: SectionFaceErase,
    pub current: SectionPickState,
    pub brush: PhysicalBrush,
}
pub(super) struct Correction {
    worker: Worker<Job, Output>,
    input: Option<Input>,
    radius: f64,
    pending: Option<Pending>,
    preview: Option<Output>,
    viewport: Option<(egui::Id, egui::LayerId, egui::Rect, egui::Rect)>,
    panel: bool,
    notice: Option<String>,
}
impl Correction {
    pub fn new(ctx: egui::Context) -> Result<Self, String> {
        Ok(Self {
            worker: Worker::start(ctx, "orv-surface-correction", run)?,
            input: None,
            radius: 5.,
            pending: None,
            preview: None,
            viewport: None,
            panel: true,
            notice: None,
        })
    }
    pub fn clear(&mut self) {
        self.worker.clear();
        self.pending = None;
        self.preview = None;
        self.notice = None;
    }
    pub fn sync(&mut self, input: Option<Input>) {
        let same = matches!((&self.input, &input), (None, None))
            || matches!((&self.input,&input),(Some(a),Some(b)) if a.same(b));
        if !same {
            self.clear();
            self.input = input;
        }
    }
    pub fn open_panel(&mut self) {
        self.clear();
        self.panel = true;
    }
    pub fn notice(&mut self, notice: String) {
        self.clear();
        self.notice = Some(notice);
        self.panel = true;
    }
    fn accepts(&self, output: &Output, current: &SectionPickState) -> bool {
        self.input.as_ref().is_some_and(|input| {
            input.matches(current)
                && output.proposal.is_current(
                    current,
                    &input.brush(self.radius, current),
                    output.proposal.proposal().points_mm(),
                )
        })
    }
    pub fn show(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        live: Option<(SectionPickFrame, SectionPickState)>,
        engine: Arc<Mutex<Option<Engine>>>,
        interacting: bool,
    ) {
        let live = live.filter(|(frame, current)| current.matches(frame));
        let viewport = (
            response.id,
            response.layer_id,
            response.rect,
            ui.clip_rect(),
        );
        let blocked = interacting
            || response.double_clicked()
            || !ui.is_enabled()
            || !response.enabled()
            || egui::Popup::is_any_open(ui.ctx())
            || !ui.input(|i| i.focused && !i.key_pressed(egui::Key::Escape))
            || !ui.ctx().memory(|m| m.allows_interaction(response.layer_id));
        if blocked || self.viewport.is_some_and(|old| old != viewport) {
            self.clear();
        }
        self.viewport = Some(viewport);
        let current = live.as_ref().map(|(_, current)| current);
        if current.is_none()
            || self
                .input
                .as_ref()
                .is_none_or(|input| current.is_none_or(|s| !input.matches(s)))
        {
            self.clear();
        }
        if let Some((generation, result)) = self.worker.take()
            && let Some(pending) = self.pending.take()
            && generation == pending.generation
            && pending.viewport == viewport
            && self
                .input
                .as_ref()
                .is_some_and(|input| input.same(&pending.input))
            && let Some(current) = current
        {
            match result {
                Ok(output)
                    if self.accepts(&output, current)
                        && pending.brush == pending.input.brush(self.radius, current) =>
                {
                    self.preview = Some(output);
                    self.panel = true;
                    self.notice = None;
                }
                Err(error) if current.scope.is_active() => {
                    self.notice = Some(error);
                    self.panel = true;
                }
                _ => (),
            }
        }
        if self
            .preview
            .as_ref()
            .is_some_and(|output| current.is_none_or(|s| !self.accepts(output, s)))
        {
            self.clear();
        }
        let ready = !blocked
            && current.is_some_and(|s| self.input.as_ref().is_some_and(|input| input.matches(s)));
        if ready && !self.panel && response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        if ready
            && !self.panel
            && response.clicked_by(egui::PointerButton::Primary)
            && !response.double_clicked()
            && !crate::viewport::navigation_override(ui, response)
            && ui.input(|i| i.modifiers == egui::Modifiers::NONE)
            && let (Some(input), Some((frame, current))) = (self.input.clone(), live)
            && let Some(pixel) = response.interact_pointer_pos().and_then(|point| {
                super::super::regions::picking::pixel_at(response.rect, point, current.scene.size)
            })
        {
            self.clear();
            let brush = input.brush(self.radius, &current);
            let generation = self.worker.submit(Job {
                engine,
                frame,
                current,
                pixel,
                brush: brush.clone(),
            });
            self.pending = Some(Pending {
                generation,
                input,
                brush,
                viewport,
            });
        }
        let compact = response.rect.width() < 240.;
        let notice = if self.pending.is_some() && compact {
            "Checking cells…".into()
        } else if self.pending.is_some() {
            "Checking visible cells…".into()
        } else if self.panel && compact {
            "Review stamp".into()
        } else if self.panel {
            "Surface erase stamp · review the tool window".into()
        } else if !ready && compact {
            "Wait for view…".into()
        } else if !ready {
            "Surface erase stamp · wait for the current manual region view".into()
        } else if compact {
            format!("Erase {:.1} mm · click", self.radius)
        } else {
            format!(
                "Erase one visible layer · {:.1} mm · click a cell face",
                self.radius
            )
        };
        let painter = ui.painter_at(response.rect);
        let text = painter.layout(
            notice,
            egui::FontId::monospace(10.),
            crate::theme::PAPER,
            (response.rect.width() - 12.).max(16.),
        );
        let origin = response.rect.left_bottom() + egui::vec2(6., -6. - text.size().y);
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
    pub fn window(
        &mut self,
        ctx: &egui::Context,
        enabled: &mut bool,
        live: Option<(SectionPickFrame, SectionPickState)>,
    ) -> Option<Apply> {
        if !*enabled {
            self.clear();
            return None;
        }
        if !ctx.input(|i| i.focused) {
            self.clear();
        }
        let current = live
            .filter(|(frame, current)| current.matches(frame))
            .map(|(_, current)| current);
        if self
            .preview
            .as_ref()
            .is_some_and(|p| current.as_ref().is_none_or(|s| !self.accepts(p, s)))
        {
            self.clear();
        }
        let mut open = *enabled;
        crate::dialogs::escape(ctx, &mut open);
        if !open {
            self.clear();
            *enabled = false;
            return None;
        }
        if !self.panel {
            return None;
        }
        let mut choose = false;
        let mut apply = false;
        let mut cancel = false;
        crate::dialogs::window("Surface erase stamp",ctx).open(&mut open).default_width(400.).show(ctx,|ui|{
            ui.label("Erase one visible source-cell layer of the selected manual label. Each click prepares a private preview.");
            let changed=crate::toolbar::number(ui,"Radius (mm)",egui::DragValue::new(&mut self.radius).range(0.1..=50.).speed(0.1).fixed_decimals(1)).changed();
            if changed {self.clear();}
            if self.input.is_none() {ui.small("Use an applied manual label map, show its selected ID in Labels, and close other label tools.");}
            let ready = current.as_ref().is_some_and(|current| self.input.as_ref().is_some_and(|input|input.matches(current)));
            if self.input.is_some() && !ready {ui.small("Choose Manual in Region in 3D and wait for the completed view.");}
            if let Some(notice)=&self.notice {ui.label(notice);}
            if let Some(output)=&self.preview {
                ui.label(format!("Preview only · {} cells will be erased",output.proposal.summary().changed_voxels));
                let size=output.image.scene().size;
                let aspect=size[0]as f32/size[1]as f32;
                let width=ui.available_width().min(360.);
                let max_height=(ctx.content_rect().height()-540.).clamp(80.,280.);
                let height=(width/aspect).min(max_height);
                let (rect,_)=ui.allocate_exact_size(egui::vec2(height*aspect,height),egui::Sense::hover());
                ui.painter_at(rect).add(egui_wgpu::Callback::new_paint_callback(rect,Paint{image:output.image.clone()}));
                ui.small("The main view still shows the applied mask. Apply changes labels in memory and adds one label undo step when cells change.");
                ui.horizontal_wrapped(|ui|{
                    apply=ui.button("Apply erase").clicked();
                    cancel=ui.button("Cancel preview").clicked();
                });
            } else {
                ui.small("Choose a face, then click the main 3D view. Covered and ambiguous faces cannot anchor a stamp. Cell centres define the physical radius; hidden layers stay unchanged.");
                choose=ui.add_enabled(ready,egui::Button::new("Choose a face")).clicked();
            }
            crate::dialogs::reveal_focus(ui);
        });
        if !open {
            self.clear();
            *enabled = false;
            return None;
        }
        if choose || cancel {
            self.clear();
            self.panel = false;
        }
        if apply
            && let (Some(output), Some(current)) = (self.preview.take(), current)
            && self.accepts(&output, &current)
            && let Some(input) = &self.input
        {
            self.panel = false;
            return Some(Apply {
                proposal: output.proposal,
                brush: input.brush(self.radius, &current),
                current,
            });
        }
        None
    }
}
fn run(job: Job, cancel: &dyn Fn() -> bool) -> Result<Output, String> {
    let obsolete = || cancel() || !job.current.scope.is_active();
    if obsolete() {
        return Err("Cancelled".into());
    }
    let mut engine = job
        .engine
        .lock()
        .map_err(|_| "Surface correction renderer unavailable")?;
    let engine = engine
        .as_mut()
        .ok_or("Surface correction renderer unavailable")?;
    let (proposal, _) = engine
        .renderer
        .propose_erase_stamp(&job.frame, &job.current, job.pixel, &job.brush, obsolete)
        .map_err(str::to_owned)?;
    if proposal.summary().changed_voxels == 0 {
        return Ok(Output {
            proposal,
            image: job.frame.image().clone(),
        });
    }
    let mut faces = engine
        .faces
        .as_ref()
        .ok_or("Current cell-face cache unavailable")?
        .clone();
    let selection = RegionSelection::new(
        crate::labels::RegionMap::Manual(proposal.proposal().preview().clone()),
        job.current.number,
    )
    .map_err(str::to_owned)?;
    let projection = faces.project(
        &selection,
        &job.current.scene.navigation,
        &job.current.scene.volume,
        job.current.scene.size,
        &obsolete,
        &obsolete,
    )?;
    let mut preview = job.current.scene.clone();
    let surface = preview
        .surface
        .as_mut()
        .ok_or("Current region surface unavailable")?;
    surface.image = projection;
    let image = engine.compose_prepared(&preview, &obsolete)?;
    if obsolete() || !proposal.is_current(&job.current, &job.brush, proposal.proposal().points_mm())
    {
        return Err("Cancelled".into());
    }
    Ok(Output { proposal, image })
}
