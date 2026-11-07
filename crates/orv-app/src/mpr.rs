// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{
    annotations::Annotations,
    controls,
    mpr_worker::{MprSnapshot, MprWorker, PLANES},
    theme,
    viewport::Tool,
};
use eframe::egui::{self, Rect, RichText, Vec2};
use orv_core::{ViewState, WindowLevel};

#[derive(Clone, Copy, PartialEq, Default)]
enum Layout {
    #[default]
    Grid,
    Columns,
    Workbench,
    Axial,
    Coronal,
    Sagittal,
}
impl Layout {
    fn label(self) -> &'static str {
        match self {
            Self::Grid => "Adaptive grid",
            Self::Columns => "Three columns",
            Self::Workbench => "MPR + 3D",
            Self::Axial => "Focus axial",
            Self::Coronal => "Focus coronal",
            Self::Sagittal => "Focus sagittal",
        }
    }
}
pub struct MprSession {
    pub(super) picture_sizes: [Option<[u32; 2]>; 3],
    worker: MprWorker,
    pub(super) snapshot: Option<MprSnapshot>,
    error: Option<String>,
    pub(super) target: Option<[f64; 3]>,
    pub(super) views: [ViewState; 3],
    pub(super) annotations: [Annotations; 3],
    pub(super) scroll: [f32; 3],
    pub(super) active: usize,
    pub(super) tool: Tool,
    pub(super) window: WindowLevel,
    original_window: WindowLevel,
    invert: bool,
    layout: Layout,
    reformat_side: u32,
    last_input: std::time::Instant,
    slab: Option<orv_core::slab::Slab>,
    workbench: Option<crate::volume_ui::VolumeSession>,
    prefer_source: bool,
    #[cfg(feature = "qa")]
    pub(crate) qa: crate::qa::MprTracking,
}
impl MprSession {
    pub(super) fn set_active(&mut self, active: bool) {
        self.worker.set_suspended(!active);
        if !active {
            #[cfg(feature = "qa")]
            self.qa.suspend();
            self.workbench = None;
            self.snapshot = None;
            self.picture_sizes = [None; 3];
        }
    }
    pub(super) fn offer_gpu(
        &self,
        device: &eframe::egui_wgpu::wgpu::Device,
        queue: &eframe::egui_wgpu::wgpu::Queue,
    ) {
        #[cfg(feature = "qa")]
        if option_env!("ORV_QA_MPR_CPU") == Some("1") {
            return;
        }
        self.worker.offer_gpu(device, queue);
    }
    pub(super) fn active_index(&self) -> usize {
        match self.layout {
            Layout::Axial => 0,
            Layout::Coronal => 1,
            Layout::Sagittal => 2,
            _ => self.active,
        }
    }
    pub(crate) fn picture(&self) -> Option<(crate::export::Pixels, [u32; 2])> {
        if !self.is_settled() {
            return None;
        }
        let index = self.active_index();
        Some((
            crate::export::Pixels::Reformat(
                self.snapshot.as_ref()?.images[index].clone(),
                self.snapshot.as_ref()?.volume.clone(),
            ),
            self.picture_sizes[index]?,
        ))
    }
    pub fn start(
        frames: impl Into<crate::frame_source::FrameSource>,
        window: WindowLevel,
        ctx: egui::Context,
    ) -> Self {
        let frames = frames.into();
        let original_window = frames.original_window().unwrap_or(window);
        let prefer_source = matches!(&frames, crate::frame_source::FrameSource::Indexed(series) if series.modality != "CT");
        Self {
            picture_sizes: [None; 3],
            worker: MprWorker::start(frames, ctx),
            snapshot: None,
            error: None,
            target: None,
            views: std::array::from_fn(|i| ViewState {
                plane: PLANES[i],
                window,
                ..Default::default()
            }),
            annotations: std::array::from_fn(|_| Annotations::for_reformats()),
            scroll: [0.; 3],
            active: 0,
            tool: Tool::Crosshair,
            window,
            original_window,
            invert: false,
            layout: Layout::Grid,
            reformat_side: 512,
            last_input: std::time::Instant::now(),
            slab: None,
            workbench: None,
            prefer_source,
            #[cfg(feature = "qa")]
            qa: Default::default(),
        }
    }
    pub(super) fn request(&mut self, volume: &orv_core::volume::Volume, point: [f64; 3]) {
        let Some(point) = volume.clamp_world(point) else {
            return;
        };
        if self
            .target
            .is_some_and(|old| old.iter().zip(point).all(|(a, b)| (a - b).abs() < 1e-6))
        {
            return;
        }
        self.target = Some(point);
        self.last_input = std::time::Instant::now();
        self.worker
            .request_slab(point, self.reformat_side, self.slab);
        #[cfg(feature = "qa")]
        self.qa.request(self.worker.generation(), self.last_input);
    }
    pub(super) fn is_settled(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|s| !s.preview && s.generation == self.worker.generation())
    }
    #[cfg(feature = "mcp")]
    pub(super) fn set_canonical(&mut self, canonical: bool) {
        self.worker.set_canonical(canonical && !self.is_workbench());
    }
    pub(super) fn poll(&mut self) {
        if let Some(result) = self.worker.take() {
            match result {
                Ok(snapshot) => {
                    #[cfg(feature = "qa")]
                    self.qa.snapshot(
                        snapshot.generation,
                        self.worker.generation(),
                        std::time::Instant::now(),
                        snapshot.preview,
                        snapshot.images[0].slab.is_some(),
                        snapshot.gpu_images.iter().all(Option::is_some),
                    );
                    if snapshot.volume.is_color() {
                        self.window = snapshot.volume.frames()[0].default_window;
                    }
                    if self.snapshot.is_none()
                        && let Some(frame) = snapshot
                            .volume
                            .frames()
                            .get(snapshot.volume.frames().len() / 2)
                    {
                        self.original_window = frame.default_window;
                    }
                    if self.target.is_none() {
                        self.target = Some(snapshot.point);
                    }
                    self.snapshot = Some(snapshot);
                    self.error = None;
                }
                Err(error) => self.error = Some(error),
            }
        }
    }
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        frame: &eframe::Frame,
        labels: &mut crate::labels::LabelUi,
        selected: usize,
    ) {
        let content_width = ui.available_width();
        if let Some(render) = frame.wgpu_render_state() {
            self.offer_gpu(&render.device, &render.queue);
        }
        self.poll();
        if let Some(error) = &self.error {
            ui.add_space(20.);
            ui.heading("This series cannot be reformatted");
            ui.label(error);
            ui.label("You can continue exploring the images in 2D.");
            return;
        }
        let Some(snapshot) = self.snapshot.clone() else {
            ui.add_space(30.);
            ui.label("Checking volume geometry and preparing linked views…");
            ui.label(RichText::new("Images remain on this device.").color(theme::SILVER));
            return;
        };
        if self.is_workbench() {
            labels.sync_physical(&self.physical_context(&snapshot, selected));
        } else {
            labels.end_physical_tools();
        }
        let color = snapshot.volume.is_color();
        if color && self.tool == Tool::Window {
            self.tool = Tool::Pan;
        }
        crate::actions::tool_input(
            ui,
            &mut self.tool,
            !color,
            true,
            snapshot.images[0].slab.is_none() && self.slab.is_none(),
        );
        if crate::actions::pressed(ui, crate::actions::Action::Invert) {
            self.invert = !self.invert;
        }
        if crate::actions::pressed(ui, crate::actions::Action::Fit) {
            for view in &mut self.views {
                view.zoom = 1.;
                view.pan = [0.; 2];
            }
        }
        crate::view_context::show(ui, &snapshot.volume.frames()[0]);
        for (index, image) in snapshot.images.iter().enumerate() {
            self.annotations[index].bind_image(image, &self.views[index]);
        }
        if self.slab.is_some() && self.tool.measures() {
            self.tool = Tool::Pan;
        }
        crate::toolbar::row(ui, |ui| {
            if ui.button("Labels…").clicked() {
                labels.open = true;
            }
            egui::ComboBox::from_id_salt("mpr-layout")
                .wrap_mode(egui::TextWrapMode::Truncate)
                .width(150.)
                .selected_text(self.layout.label())
                .show_ui(ui, |ui| {
                    for layout in [
                        Layout::Grid,
                        Layout::Columns,
                        Layout::Workbench,
                        Layout::Axial,
                        Layout::Coronal,
                        Layout::Sagittal,
                    ] {
                        ui.selectable_value(&mut self.layout, layout, layout.label());
                    }
                });
            let label = ui.label("Active plane");
            let active = self.active_index();
            ui.add_enabled_ui(
                matches!(
                    self.layout,
                    Layout::Grid | Layout::Columns | Layout::Workbench
                ),
                |ui| {
                    egui::ComboBox::from_id_salt("active-mpr-plane")
                        .width(85.)
                        .selected_text(["Axial", "Coronal", "Sagittal"][active])
                        .show_ui(ui, |ui| {
                            for (index, name) in
                                ["Axial", "Coronal", "Sagittal"].into_iter().enumerate()
                            {
                                if ui.selectable_value(&mut self.active, index, name).clicked() {
                                    ui.close();
                                }
                            }
                        })
                        .response
                        .labelled_by(label.id);
                },
            );
            egui::CollapsingHeader::new("Point coordinates").show(ui, |ui| {
                    let [min, max] = snapshot.volume.bounds();
                    let mut point = self.target.unwrap_or(snapshot.point);
                    let previous = point;
                    for (axis, name) in ["L (mm)", "P (mm)", "S (mm)"].into_iter().enumerate() {
                        crate::toolbar::number(ui, name, egui::DragValue::new(&mut point[axis])
                            .range(min[axis]..=max[axis]).max_decimals(3).speed(snapshot.volume.step_mm()));
                    }
                    ui.weak("Source LPS coordinates: left, posterior, superior. No body landmark is inferred.");
                    if point != previous { self.request(&snapshot.volume, point); }
                });
        });
        let workbench = self.is_workbench();
        let mut display_controls = |ui: &mut egui::Ui| {
            let active = self.active_index();
            if controls::tools_with_measurements(
                ui,
                &mut self.tool,
                true,
                !color,
                snapshot.images[0].slab.is_none() && self.slab.is_none(),
                |ui| {
                    controls::orientation(ui, &mut self.views[active].transform);
                },
            ) {
                for view in &mut self.views {
                    view.zoom = 1.;
                    view.pan = [0.; 2];
                }
            }
            crate::toolbar::row(ui, |ui| {
                let active = self.active_index();
                self.annotations[active].controls(ui);
                if ui.button("Clear all panes").clicked() {
                    for annotations in &mut self.annotations {
                        annotations.clear();
                    }
                }
            });
            if color {
                controls::color_display(ui, &mut self.invert, snapshot.volume.color_info());
            } else {
                controls::window(
                    ui,
                    &mut self.window,
                    &mut self.invert,
                    self.original_window,
                    snapshot.volume.frames()[0]
                        .scalar
                        .as_ref()
                        .map(|data| &**data.choices()),
                );
            }
        };
        if workbench {
            egui::CollapsingHeader::new("MPR display and tools")
                .id_salt("workbench-mpr-controls")
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("workbench-mpr-controls-scroll")
                        .max_height((ui.available_height() * 0.35).clamp(80., 180.))
                        .show(ui, &mut display_controls);
                });
        } else {
            controls::display_panel(ui, "mpr-display-controls", display_controls);
        }
        if !color && crate::slab_controls::show(ui, &mut self.slab, &snapshot.volume) {
            let point = self.target.unwrap_or(snapshot.point);
            self.target = None;
            self.request(&snapshot.volume, point);
        }
        let pending = self.worker.generation() != snapshot.generation || snapshot.preview;
        if self.tool != Tool::Crosshair
            || (!labels.physical_dialog_open()
                && ui.input(|input| input.key_pressed(egui::Key::Escape)))
        {
            labels.stop_physical_drawing();
        }
        let elapsed = self.last_input.elapsed();
        let sampling_status =
            crate::slab_controls::pending_sampling(snapshot.images[0].slab, self.slab);
        let show_pending = pending && elapsed >= std::time::Duration::from_millis(250);
        crate::slab_controls::status_line(
            ui,
            sampling_status
                .as_deref()
                .or(show_pending.then_some("Updating linked views…")),
        );
        if sampling_status.is_none() && pending && elapsed < std::time::Duration::from_millis(250) {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250) - elapsed);
        }
        let size = Vec2::new(content_width, ui.available_height().max(60.));
        let (area, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        if !self.is_workbench() {
            self.workbench = None;
        }
        #[cfg(feature = "mcp")]
        if self.is_workbench() {
            self.set_canonical(false);
        }
        let gap = 12.0;
        let mut paints = Vec::with_capacity(3);
        let layout = if self.layout == Layout::Grid && area.height() < 360. {
            Layout::Columns
        } else {
            self.layout
        };
        match layout {
            Layout::Columns => {
                for i in 0..3 {
                    let width = (area.width() - gap * 2.) / 3.;
                    let rect = Rect::from_min_size(
                        area.min + Vec2::new(i as f32 * (width + gap), 0.),
                        Vec2::new(width, area.height()),
                    );
                    paints.push(self.prepare_plane(ui, i, rect, &snapshot, labels, selected));
                }
            }
            Layout::Workbench => {
                let rects = workbench_rects(area);
                for (index, rect) in rects[..3].iter().enumerate() {
                    paints.push(self.prepare_plane(ui, index, *rect, &snapshot, labels, selected));
                }
                labels.sync_physical(&self.physical_context(&snapshot, selected));
                let rect = rects[3];
                let mut view = ui.new_child(
                    egui::UiBuilder::new()
                        .id_salt("mpr-workbench")
                        .max_rect(rect),
                );
                view.set_clip_rect(rect);
                self.workbench_ui(&mut view, frame, &snapshot, labels, selected);
            }
            Layout::Grid => {
                let cell = (area.size() - Vec2::splat(gap)) / 2.;
                for i in 0..3 {
                    let offset = Vec2::new(
                        (i % 2) as f32 * (cell.x + gap),
                        (i / 2) as f32 * (cell.y + gap),
                    );
                    paints.push(self.prepare_plane(
                        ui,
                        i,
                        Rect::from_min_size(area.min + offset, cell),
                        &snapshot,
                        labels,
                        selected,
                    ));
                }
                let rect = Rect::from_min_size(area.min + cell + Vec2::splat(gap), cell);
                let mut info =
                    ui.new_child(egui::UiBuilder::new().id_salt("mpr-guide").max_rect(rect));
                info.set_clip_rect(rect);
                self.guide(&mut info, &snapshot);
            }
            focus => {
                let index = match focus {
                    Layout::Coronal => 1,
                    Layout::Sagittal => 2,
                    _ => 0,
                };
                paints.push(self.prepare_plane(ui, index, area, &snapshot, labels, selected));
            }
        }
        // Apply the final shared display state after handling all panes' input.
        // Each callback owns its own texture/uniform slot; no late write can replace a sibling's data.
        for view in &mut self.views {
            view.window = self.window;
            view.invert = self.invert;
        }
        if self.is_workbench() {
            labels.sync_physical(&self.physical_context(&snapshot, selected));
        }
        labels.prepare_planes(
            selected,
            &snapshot.volume,
            &snapshot.images,
            crate::labels::OverlayView::Mpr {
                generation: snapshot.generation,
                preview: snapshot.preview,
            },
            ui.ctx(),
        );
        for paint in paints {
            self.finish_plane(paint, frame, &snapshot, labels);
        }
        if self.is_workbench() {
            labels.physical_window(ui.ctx(), &self.physical_context(&snapshot, selected));
            if labels.take_physical_draw_request() {
                self.tool = Tool::Crosshair;
            }
        }
    }
    pub(super) fn physical_context<'a>(
        &self,
        snapshot: &'a MprSnapshot,
        selected: usize,
    ) -> crate::labels::PhysicalContext<'a> {
        crate::labels::PhysicalContext {
            source: &snapshot.volume,
            selected,
            point: snapshot.point,
            plane: self.active_index(),
            generation: self.worker.generation(),
            ready: self.worker.generation() == snapshot.generation
                && !snapshot.preview
                && self.slab.is_none()
                && snapshot.images.iter().all(|image| image.slab.is_none()),
        }
    }
    pub(crate) fn controls_open(&self) -> bool {
        self.workbench
            .as_ref()
            .is_some_and(|workbench| workbench.controls_open())
    }
    pub(crate) fn is_workbench(&self) -> bool {
        self.layout == Layout::Workbench
    }
    #[cfg(all(test, feature = "mcp"))]
    pub(crate) fn enable_test_workbench(&mut self) {
        self.layout = Layout::Workbench;
    }
    fn workbench_ui(
        &mut self,
        ui: &mut egui::Ui,
        frame: &eframe::Frame,
        snapshot: &MprSnapshot,
        labels: &mut crate::labels::LabelUi,
        selected: usize,
    ) {
        if self.workbench.is_none() {
            let source = snapshot
                .gpu_images
                .iter()
                .flatten()
                .next()
                .map(|image| image.source().clone());
            // An older CPU snapshot can still be visible while the first GPU
            // offer is uploading. Wait rather than allocate a second source.
            if source.is_none() && snapshot.generation != self.worker.generation() {
                ui.label("Waiting for the current MPR source…");
                return;
            }
            let Some(render) = frame.wgpu_render_state() else {
                ui.label("3D requires the native GPU renderer");
                return;
            };
            self.workbench = Some(crate::volume_ui::VolumeSession::from_mpr(
                snapshot.volume.clone(),
                source,
                self.prefer_source,
                render,
                ui.ctx().clone(),
            ));
        }
        let link_epoch = self.worker.generation();
        let planes_current = self.is_settled();
        let picked = self.workbench.as_mut().and_then(|workbench| {
            workbench.embedded_ui(
                ui,
                labels,
                selected,
                snapshot.point,
                link_epoch,
                Some(crate::volume_ui::LinkedPlanes {
                    images: &snapshot.gpu_images,
                    window: self.window,
                    invert: self.invert,
                    current: planes_current,
                }),
            )
        });
        if let Some(point) = picked {
            self.request(&snapshot.volume, point);
        }
    }
    fn guide(&self, ui: &mut egui::Ui, snapshot: &MprSnapshot) {
        theme::eyebrow(ui, "LINKED CROSSHAIR · mm (LPS)");
        ui.label(format!(
            "L {:.2}   P {:.2}   S {:.2}",
            snapshot.point[0], snapshot.point[1], snapshot.point[2]
        )).on_hover_text("Coordinates in the source LPS reference, in millimetres: left, posterior, superior. Negative values are valid and depend on the source origin; these are not distances from a body landmark.");
        ui.add_space(8.);
        ui.label(format!("Crosshair: choose the same point in all views. {} or ↑↓: move a plane. {} or Space drag: move the image.",
            crate::actions::label(ui.ctx(), crate::actions::Action::Browse),
            crate::actions::label(ui.ctx(), crate::actions::Action::Pan)));
        controls::navigation_hint(ui, self.tool);
        ui.label(RichText::new("Measurements on resampled images may differ from the source images. ROI values follow the selected scalar or RGB representation.").small().color(theme::SILVER));
        let [x, y, z] = snapshot.volume.dimensions();
        ui.label(
            RichText::new(format!(
                "{x} × {y} × {z} source samples\nPersonal research only"
            ))
            .monospace()
            .size(10.)
            .color(theme::SILVER),
        );
    }

    pub(super) fn rendered_view(&self) -> Option<(crate::rendered::ViewSpec, bool)> {
        if self.is_workbench() {
            return None;
        }
        let snapshot = self.snapshot.as_ref()?;
        let index = self.active_index();
        let mut view = self.views[index].clone();
        view.slice = 0;
        let reconstruction = if let Some(slab) = snapshot.images[index].slab {
            view.plane = orv_core::Plane::Acquired;
            Some(orv_core::scene::Reconstruction::Slab(
                orv_core::slab::SlabPlane {
                    plane: orv_core::oblique::ObliquePlane::anatomical(
                        PLANES[index],
                        snapshot.point,
                    )
                    .ok()?,
                    slab,
                },
            ))
        } else {
            None
        };
        let point = if reconstruction.is_some() {
            None
        } else {
            Some(snapshot.point)
        };
        Some((
            crate::rendered::ViewSpec {
                view,
                point,
                reformat_side: snapshot.reformat_side,
                reconstruction,
                preferred_size: None,
            },
            self.is_settled(),
        ))
    }

    #[cfg(feature = "mcp")]
    pub(super) fn apply_shared_view(&mut self, scene: &orv_core::scene::SceneState) {
        use orv_core::Plane;
        self.active = match scene.view.plane {
            Plane::Coronal => 1,
            Plane::Sagittal => 2,
            _ => 0,
        };
        self.layout = match self.active {
            1 => Layout::Coronal,
            2 => Layout::Sagittal,
            _ => Layout::Axial,
        };
        self.window = scene.view.window;
        self.invert = scene.view.invert;
        self.views[self.active] = scene.view.clone();
        self.reformat_side = scene.reformat_side;
        self.slab = None;
        if let Some(point) = scene.crosshair_mm {
            self.target = Some(point);
            self.last_input = std::time::Instant::now();
            self.worker.request(point, scene.reformat_side);
        }
    }
}

fn workbench_rects(area: Rect) -> [Rect; 4] {
    let gap = 12.;
    let columns = area.height() < 360.;
    let cell = if columns {
        Vec2::new((area.width() - gap * 3.) / 4., area.height())
    } else {
        (area.size() - Vec2::splat(gap)) / 2.
    };
    std::array::from_fn(|index| {
        let [column, row] = if columns {
            [index, 0]
        } else {
            [index % 2, index / 2]
        };
        Rect::from_min_size(
            area.min + Vec2::new(column as f32 * (cell.x + gap), row as f32 * (cell.y + gap)),
            cell,
        )
    })
}

#[cfg(test)]
mod layout_tests {
    use super::*;
    #[test]
    fn short_workbench_uses_the_full_height_and_all_cells_remain_disjoint() {
        for size in [egui::vec2(608., 180.), egui::vec2(1000., 430.)] {
            let area = Rect::from_min_size(egui::pos2(236., 320.), size);
            let cells = workbench_rects(area);
            for (index, cell) in cells.iter().enumerate() {
                assert!(area.contains_rect(*cell));
                assert!(
                    cell.width() >= 140. && cell.height() >= 180.,
                    "Insufficient image space: {cell:?}"
                );
                assert!(cells[..index].iter().all(|other| !cell.intersects(*other)));
            }
            if size.y < 360. {
                assert!(cells.iter().all(|cell| cell.height() == size.y));
            }
        }
    }
    #[test]
    fn pending_linked_views_do_not_shift_the_panes_or_adaptive_layout() {
        for size in [egui::vec2(860., 640.), egui::vec2(1280., 840.)] {
            let ctx = egui::Context::default();
            let mut panes = Vec::new();
            for pending in [false, true, false, true] {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, size)),
                        ..Default::default()
                    },
                    |ui| {
                        crate::slab_controls::status_line(
                            ui,
                            pending.then_some("Showing MIP (80 mm, 251 samples); updating…"),
                        );
                        let (rect, _) =
                            ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
                        panes.push(rect);
                    },
                );
                output.textures_delta.clear();
            }
            assert!(panes.windows(2).all(|p| p[0] == p[1]));
        }
    }
}

#[cfg(all(test, feature = "mcp"))]
mod preview_sharing_tests {
    use super::*;
    #[test]
    fn interactive_preview_is_withheld_from_exact_scene_publication_until_refined() {
        let mut session = MprSession::start(
            crate::mpr_worker::tests::frames(),
            WindowLevel::SOFT_TISSUE,
            egui::Context::default(),
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while session.snapshot.is_none() {
            session.poll();
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        let volume = session.snapshot.as_ref().unwrap().volume.clone();
        session.request(&volume, [1., 2., 9.]);
        while !session.snapshot.as_ref().unwrap().preview {
            session.poll();
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        let (_, ready) = session.rendered_view().unwrap();
        assert!(!ready);
        while !session.is_settled() {
            session.poll();
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        let (spec, ready) = session.rendered_view().unwrap();
        assert!(ready);
        assert_eq!(spec.reformat_side, 512);
        assert_eq!(spec.point, Some([1., 2., 9.]));
    }
}
