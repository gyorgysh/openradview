// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{export::worker::Task, jobs::Worker};
use orv_core::resources::MemoryBudget;

#[derive(Default)]
pub(in crate::export) struct Controller {
    open: bool,
    input: Option<Arc<FrozenInput>>,
    choice: Choice,
    worker: Option<Worker<Request, Arc<Artifact>>>,
    pending: Option<u64>,
    artifact: Option<Arc<Artifact>>,
    texture: Option<(egui::TextureHandle, Reservation)>,
    saving: Option<Task>,
    notice: Option<String>,
}
impl Controller {
    pub fn is_open(&self) -> bool {
        self.open
    }
    pub fn open(&mut self, input: Result<FrozenInput, &'static str>) {
        *self = Self {
            open: true,
            ..Default::default()
        };
        match input {
            Ok(input) => self.input = Some(Arc::new(input)),
            Err(error) => self.notice = Some(error.into()),
        }
    }
    fn invalidate(&mut self) {
        if let Some(worker) = &self.worker {
            worker.clear();
        }
        self.pending = None;
        self.artifact = None;
        self.texture = None;
        self.notice = None;
    }
    fn matches(&self, artifact: &Artifact) -> bool {
        self.input
            .as_ref()
            .is_some_and(|input| Arc::ptr_eq(input, &artifact.input))
            && self.choice == artifact.choice
    }
    fn choose(&mut self, choice: Choice) {
        if choice != self.choice {
            self.choice = choice;
            self.invalidate();
        }
    }
    fn accept(
        &mut self,
        ctx: &egui::Context,
        generation: u64,
        result: Result<Arc<Artifact>, String>,
    ) {
        if self.pending != Some(generation) {
            return;
        }
        self.pending = None;
        match result {
            Ok(artifact) if self.matches(&artifact) => {
                let limit = ctx.input(|input| input.max_texture_side);
                if artifact.size.iter().any(|n| *n as usize > limit) {
                    self.notice = Some("The preview exceeds this device's texture limit".into());
                } else if let Some(memory) = MemoryBudget::gpu()
                    .reserve(u64::from(artifact.size[0]) * u64::from(artifact.size[1]) * 4)
                {
                    let texture = ctx.load_texture(
                        "reviewed-illustration",
                        egui::ImageData::Color(artifact.preview.clone()),
                        egui::TextureOptions::NEAREST,
                    );
                    self.texture = Some((texture, memory));
                    self.artifact = Some(artifact);
                } else {
                    self.notice = Some("Not enough GPU allowance to show the preview".into());
                }
            }
            Ok(_) => {}
            Err(error) => self.notice = Some(error),
        }
    }
    fn poll(&mut self, ctx: &egui::Context) {
        if let Some((generation, result)) = self.worker.as_ref().and_then(Worker::take) {
            self.accept(ctx, generation, result);
        }
        if let Some(result) = self.saving.as_ref().and_then(Task::poll) {
            self.notice = Some(result.unwrap_or_else(|message| message).into());
            self.saving = None;
        }
    }
    fn prepare(&mut self, ctx: &egui::Context) {
        self.invalidate();
        if self.worker.is_none() {
            match Worker::start(ctx.clone(), "orv-illustration-preview", render::prepare) {
                Ok(worker) => self.worker = Some(worker),
                Err(error) => {
                    self.notice = Some(error);
                    return;
                }
            }
        }
        if let (Some(input), Some(worker)) = (&self.input, &self.worker) {
            self.pending = Some(worker.submit(Request {
                input: input.clone(),
                choice: self.choice,
            }));
        }
    }
    pub fn show(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }
        self.poll(ctx);
        let mut open = true;
        crate::dialogs::window("Export illustration", ctx).open(&mut open)
            .default_width(570.).default_height(660.).show(ctx, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(8., 8.);
                ui.label("Choose layers, prepare a preview, then save the reviewed PNG and illustration settings locally.");
                ui.label("Identifying text may remain inside the image. Settings include source fingerprints and geometry, without source paths or patient tags. Review both files before sharing.");
                if let Some(input) = &self.input {
                    let [width, height] = input.base.scene.output_size;
                    ui.weak(format!("{width} × {height} pixels · captured current pane · personal education only"));
                    ui.weak("Results, crosshairs, scales, editing handles and app controls are excluded. Masks are a picture, not editable labels.");
                    let mut choice = self.choice;
                    ui.add_enabled_ui(self.saving.is_none(), |ui| {
                        ui.add_enabled(!input.outlines.marks().is_empty(), egui::Checkbox::new(&mut choice.marks,
                            format!("Include completed measurement outlines ({})", input.outlines.marks().len())));
                        let label = input.masks.as_ref().map_or("Include applied masks", |mask| mask.name());
                        ui.add_enabled(input.masks.is_some(), egui::Checkbox::new(&mut choice.masks, label));
                        if let Some(note) = input.mask_note { ui.weak(note); }
                    });
                    self.choose(choice);
                    if self.saving.is_none() {
                        ui.horizontal_wrapped(|ui| {
                            if ui.button("Prepare preview").clicked() { self.prepare(ctx); }
                            if self.pending.is_some() && ui.button("Cancel preview").clicked() { self.invalidate(); }
                        });
                    }
                }
                if self.pending.is_some() {
                    ui.label("Checking the source and preparing selected layers locally…");
                }
                if let (Some(artifact), Some((texture, _))) = (&self.artifact, &self.texture)
                    && self.matches(artifact)
                {
                    ui.separator();
                    ui.label("Reviewed preview");
                    let size = texture.size_vec2();
                    let scale = (ui.available_width() / size.x).min(280. / size.y).min(1.);
                    ui.image((texture.id(), size * scale));
                    ui.weak("The saved PNG uses these exact rendered bytes. This preview may be scaled to fit.");
                    if self.saving.is_none() && ui.button("Save reviewed illustration…").clicked() {
                        self.saving = Some(save::start(artifact.clone(), ctx.clone()));
                        self.notice = None;
                    }
                }
                if let Some(task) = &self.saving {
                    ui.label("Choosing a destination and rechecking the source before saving…");
                    if ui.button("Cancel save").clicked() { task.cancel(); }
                    ui.weak("A cancellation received after both files finish leaves the completed export in place.");
                }
                if let Some(notice) = &self.notice { ui.label(notice); }
            });
        if !open {
            *self = Self::default();
        }
    }
}

#[cfg(all(test, feature = "gpu-tests"))]
mod tests {
    use super::*;
    #[test]
    fn preview_acceptance_binds_capture_generation_and_choice_and_changes_clear_save_state() {
        let (device, queue) = render::tests::device();
        let input = Arc::new(render::tests::input(device.clone(), queue.clone()));
        let artifact = render::prepare(
            Request {
                input: input.clone(),
                choice: Choice::default(),
            },
            &|| false,
        )
        .unwrap();
        let ctx = egui::Context::default();
        let mut controller = Controller {
            open: true,
            input: Some(input),
            pending: Some(7),
            ..Default::default()
        };
        assert!(!controller.choice.marks && !controller.choice.masks);
        assert!(controller.artifact.is_none() && controller.texture.is_none());
        controller.accept(&ctx, 6, Ok(artifact.clone()));
        assert!(controller.artifact.is_none());
        controller.choose(Choice {
            marks: true,
            masks: false,
        });
        controller.accept(&ctx, 7, Ok(artifact.clone()));
        assert!(controller.artifact.is_none() && controller.pending.is_none());
        controller.choose(Choice::default());
        controller.input = Some(Arc::new(render::tests::input(device, queue)));
        controller.pending = Some(8);
        controller.accept(&ctx, 8, Ok(artifact.clone()));
        assert!(
            controller.artifact.is_none(),
            "An equal-looking different capture is still stale"
        );
        controller.input = Some(artifact.input.clone());
        controller.pending = Some(9);
        controller.accept(&ctx, 9, Ok(artifact));
        assert!(controller.artifact.is_some() && controller.texture.is_some());
        controller.choose(Choice {
            marks: false,
            masks: true,
        });
        assert!(controller.artifact.is_none() && controller.texture.is_none());
        controller.open(Err("No settled plane"));
        assert!(controller.input.is_none() && controller.worker.is_none());
        assert_eq!(controller.notice.as_deref(), Some("No settled plane"));
    }
}
