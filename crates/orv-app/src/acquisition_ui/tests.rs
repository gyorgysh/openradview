// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
#[path = "../../../orv-core/tests/support/nifti_axes.rs"]
mod phantom;
fn catalog(units: u32, step: f64, offset: f64) -> (tempfile::TempDir, NiftiCatalog) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("axes.nii");
    std::fs::write(&path, phantom::fixture(2, false, true, units, step, offset)).unwrap();
    let study = orv_core::index::index_paths(&[path], || false, |_| {}).unwrap();
    (dir, NiftiCatalog::prepare(&study, || false).unwrap())
}
struct Render {
    ctx: egui::Context,
    controls: Controls,
    time: f64,
    enabled: bool,
}
impl Render {
    fn new() -> Self {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        crate::theme::install(&ctx);
        Self {
            ctx,
            controls: Controls::default(),
            time: 0.,
            enabled: true,
        }
    }
    fn show(
        &mut self,
        catalog: &NiftiCatalog,
        selected: usize,
        events: Vec<egui::Event>,
    ) -> (
        Option<usize>,
        egui::accesskit::TreeUpdate,
        std::time::Duration,
    ) {
        let mut choice = None;
        self.time += 1.;
        let mut output = self.ctx.run_ui(
            egui::RawInput {
                time: Some(self.time),
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(300., 450.),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                ui.add_enabled_ui(self.enabled, |ui| {
                    choice = self.controls.show(ui, catalog, selected);
                });
            },
        );
        output.textures_delta.clear();
        (
            choice,
            output.platform_output.accesskit_update.take().unwrap(),
            output.viewport_output[&egui::ViewportId::ROOT].repaint_delay,
        )
    }
}
fn set_index(tree: &egui::accesskit::TreeUpdate, name: &str, value: f64) -> egui::Event {
    let target = tree
        .nodes
        .iter()
        .find_map(|(id, node)| {
            let named = node.label() == Some(name)
                || node.labelled_by().iter().any(|label| {
                    tree.nodes.iter().any(|(id, node)| {
                        id == label && (node.label() == Some(name) || node.value() == Some(name))
                    })
                });
            (named && node.numeric_value().is_some()).then_some(*id)
        })
        .unwrap();
    egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest {
        action: egui::accesskit::Action::SetValue,
        target_tree: egui::accesskit::TreeId::ROOT,
        target_node: target,
        data: Some(egui::accesskit::ActionData::NumericValue(value)),
    })
}
fn text(tree: &egui::accesskit::TreeUpdate) -> String {
    tree.nodes
        .iter()
        .filter_map(|(_, node)| node.label().or(node.value()))
        .collect::<Vec<_>>()
        .join("\n")
}
#[test]
fn numeric_axis_controls_select_the_exact_tuple_and_preserve_the_other_coordinates() {
    let (_dir, catalog) = catalog(18, 250., -500.);
    let mut render = Render::new();
    let mut selected = catalog.resolve(0, [0; 4], ComplexView::Real).unwrap();
    for (label, value, expected) in [
        ("Time index", 3., [2, 0, 0, 0]),
        ("Component index", 2., [2, 1, 0, 0]),
        ("Axis 6 index", 2., [2, 1, 1, 0]),
        ("Axis 7 index", 2., [2, 1, 1, 1]),
    ] {
        let (_, tree, _) = render.show(&catalog, selected, vec![]);
        let event = set_index(&tree, label, value);
        selected = render.show(&catalog, selected, vec![event]).0.unwrap();
        assert_eq!(catalog.axes(selected).unwrap().view().indices, expected);
        assert_eq!(
            catalog.axes(selected).unwrap().view().complex,
            ComplexView::Real
        );
    }
    let (_, tree, _) = render.show(&catalog, selected, vec![]);
    assert!(text(&tree).contains("Time coordinate 0 s · step 0.25 s"));
}
#[test]
fn frequency_unknown_and_invalid_timing_are_explained_without_disabling_index_selection() {
    for (units, step, offset, message) in [
        (34, 5., -10., "Frequency coordinate -10 Hz"),
        (2, 0., 0., "units are unknown"),
        (18, 0., 0., "metadata is invalid"),
    ] {
        let (_dir, catalog) = catalog(units, step, offset);
        let mut render = Render::new();
        let (_, tree, _) = render.show(&catalog, 0, vec![]);
        assert!(text(&tree).contains(message));
        let event = set_index(&tree, "Time index", 2.);
        let selected = render.show(&catalog, 0, vec![event]).0.unwrap();
        assert_eq!(catalog.axes(selected).unwrap().view().indices, [1, 0, 0, 0]);
    }
}
#[test]
fn compact_axis_controls_settle_without_an_idle_timer_and_small_steps_stay_visible() {
    let (_dir, catalog) = catalog(26, 0.000001, -0.000002);
    let mut render = Render::new();
    let mut last = None;
    for _ in 0..6 {
        last = Some(render.show(&catalog, 0, vec![]));
    }
    let (_, tree, delay) = last.unwrap();
    assert!(text(&tree).contains("e-12 s"));
    assert!(delay > std::time::Duration::from_secs(1));
    assert_eq!(number(1.), "1");
    assert_eq!(number(-0.25), "-0.25");
}

#[test]
fn disabled_axis_controls_cannot_select_a_series_through_an_accessibility_value_action() {
    let (_dir, catalog) = catalog(18, 250., -500.);
    let mut render = Render::new();
    let (_, tree, _) = render.show(&catalog, 0, vec![]);
    let event = set_index(&tree, "Time index", 3.);
    render.enabled = false;
    assert!(render.show(&catalog, 0, vec![event]).0.is_none());
}
