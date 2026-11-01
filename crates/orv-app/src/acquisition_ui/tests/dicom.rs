// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use orv_core::acquisition::{DicomAxis, DicomCatalog};
#[path = "../../../../orv-core/tests/support/dicom_axes.rs"]
mod phantom;
fn catalog(object: &dicom_object::DefaultDicomObject) -> (tempfile::TempDir, DicomCatalog) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("axes.dcm");
    object.write_to_file(&path).unwrap();
    let study = orv_core::index::index_paths(&[path], || false, |_| {}).unwrap();
    (dir, DicomCatalog::prepare(&study, || false).unwrap())
}
impl Render {
    fn dicom(
        &mut self,
        catalog: &DicomCatalog,
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
                    choice = self.controls.show_dicom(ui, catalog, selected);
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
fn click(tree: &egui::accesskit::TreeUpdate, label: &str) -> egui::Event {
    let target = tree
        .nodes
        .iter()
        .find_map(|(id, node)| (node.label() == Some(label)).then_some(*id))
        .unwrap();
    egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest {
        action: egui::accesskit::Action::Click,
        target_tree: egui::accesskit::TreeId::ROOT,
        target_node: target,
        data: None,
    })
}
#[test]
fn dicom_choices_preserve_the_other_axis_and_show_actual_values_separate_from_ordinals() {
    let (_dir, catalog) = catalog(&phantom::enhanced());
    let mut render = Render::new();
    let mut selected = 0;
    for (label, expected) in [("Position choice", [1, 2]), ("Echo choice", [1, 1])] {
        let (_, tree, _) = render.dicom(&catalog, selected, vec![]);
        let event = set_index(&tree, label, 1.);
        selected = render.dicom(&catalog, selected, vec![event]).0.unwrap();
        let axes = catalog.axes(selected).unwrap().unwrap();
        assert_eq!(
            DicomAxis::ALL.map(|axis| axes.value(axis).unwrap().ordinal),
            expected
        );
    }
    let (_, tree, _) = render.dicom(&catalog, selected, vec![]);
    let content = text(&tree);
    assert!(content.contains("Temporal position: 2 · ordinal 1"));
    assert!(content.contains("Echo time: 4.5 ms · ordinal 1"));
    assert!(!content.contains("2.25."));
}
#[test]
fn timing_inspection_reports_declared_intervals_trim_delay_and_invalid_metadata() {
    for variable in [false, true] {
        let (_dir, catalog) = catalog(&phantom::cine(variable));
        let mut render = Render::new();
        let (_, tree, _) = render.dicom(&catalog, 0, vec![]);
        let event = click(&tree, "Source timing");
        render.dicom(&catalog, 0, vec![event]);
        let (_, tree, _) = render.dicom(&catalog, 0, vec![]);
        let content = text(&tree);
        assert!(content.contains("Trim: images 2–4 of 5"));
        assert!(content.contains("Declared frame delay: 7.5 ms"));
        assert!(content.contains("manually chosen fps"));
        assert!(content.contains(if variable {
            "First to last frame: 300 ms"
        } else {
            "Interval: 50 ms"
        }));
    }
    let mut object = phantom::cine(false);
    object.put_str(dicom_core::Tag(0x0018, 0x1063), dicom_core::VR::DS, "-1");
    let (_dir, catalog) = catalog(&object);
    let mut render = Render::new();
    let (_, tree, _) = render.dicom(&catalog, 0, vec![]);
    let event = click(&tree, "Source timing");
    render.dicom(&catalog, 0, vec![event]);
    let (_, tree, _) = render.dicom(&catalog, 0, vec![]);
    assert!(text(&tree).contains("missing or invalid intervals"));
}
#[test]
fn disabled_dicom_choices_do_not_change_source_and_controls_settle_without_idle_repaints() {
    let (_dir, catalog) = catalog(&phantom::enhanced());
    let mut render = Render::new();
    let (_, tree, _) = render.dicom(&catalog, 0, vec![]);
    let event = set_index(&tree, "Position choice", 1.);
    render.enabled = false;
    assert!(render.dicom(&catalog, 0, vec![event]).0.is_none());
    render.enabled = true;
    let mut delay = std::time::Duration::ZERO;
    for _ in 0..6 {
        delay = render.dicom(&catalog, 0, vec![]).2;
    }
    assert!(delay > std::time::Duration::from_secs(1));
}
