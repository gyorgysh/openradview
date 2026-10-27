// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(crate) fn text_rect(output: &egui::FullOutput, expected: &str) -> egui::Rect {
    output
        .shapes
        .iter()
        .find_map(|s| match &s.shape {
            egui::epaint::Shape::Text(t) if t.galley.job.text == expected => {
                Some(t.galley.rect.translate(t.pos.to_vec2()))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("Missing painted control {expected}"))
}

#[test]
fn mixed_view_controls_share_the_same_text_baseline() {
    let ctx = egui::Context::default();
    crate::theme::install(&ctx);
    for _ in 0..3 {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1600., 300.),
                )),
                ..Default::default()
            },
            |ui| {
                ui.spacing_mut().button_padding = egui::vec2(10., 5.);
                ui.spacing_mut().item_spacing = egui::vec2(8., 6.);
                tools(ui, &mut Tool::Crosshair, true, true, |ui| {
                    egui::ComboBox::from_id_salt("layout-test")
                        .selected_text("Adaptive grid")
                        .show_ui(ui, |_| {});
                    let _ = ui.button("Clear measurements");
                    crate::toolbar::label(ui, egui::RichText::new("100%").small());
                });
            },
        );
        output.textures_delta.clear();
        let pan = text_rect(&output, "Pan [P]").center().y;
        for label in [
            "Crosshair",
            "Measure",
            "Fit [R]",
            "Adaptive grid",
            "Clear measurements",
            "100%",
        ] {
            let rect = text_rect(&output, label);
            assert!(
                (rect.center().y - pan).abs() < 0.75,
                "{label} is vertically displaced: {} vs {pan}",
                rect.center().y
            );
        }
    }
}

#[test]
fn window_labels_stay_with_equal_width_values_when_wrapping() {
    let ctx = egui::Context::default();
    crate::theme::install(&ctx);
    for width in [1400., 560., 300.] {
        let mut values = Vec::new();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 500.),
                )),
                ..Default::default()
            },
            |ui| {
                let mut w = WindowLevel::LUNG;
                window(ui, &mut w, &mut false, WindowLevel::LUNG, None);
                crate::toolbar::row(ui, |ui| {
                    for (label, mut v) in [("First", 1.), ("Second", 100.)] {
                        values.push(
                            crate::toolbar::number(ui, label, egui::DragValue::new(&mut v)).rect,
                        );
                    }
                });
            },
        );
        output.textures_delta.clear();
        for (label, value) in [("Width", "1500"), ("Level", "-600")] {
            let a = text_rect(&output, label);
            let b = text_rect(&output, value);
            assert!(
                (a.center().y - b.center().y).abs() < 0.75,
                "Separated {label} at {width}px"
            );
            assert!(b.right() <= width && a.left() >= 0.);
        }
        assert!((values[0].width() - values[1].width()).abs() < 0.75);
        assert!(
            values
                .iter()
                .all(|r| (r.height() - crate::toolbar::HEIGHT).abs() < 0.75)
        );
    }
}

#[test]
fn switching_from_measurement_repaints_without_another_input_and_settles_when_idle() {
    let ctx = egui::Context::default();
    crate::theme::install(&ctx);
    ctx.style_mut_of(egui::Theme::Dark, |s| s.animation_time = 0.);
    let mut tool = Tool::Distance;
    let mut time = 0.;
    let mut draw = |events: Vec<egui::Event>| {
        time += 0.1;
        let mut output = ctx.run_ui(
            egui::RawInput {
                time: Some(time),
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400., 300.),
                )),
                ..Default::default()
            },
            |ui| {
                tools(ui, &mut tool, true, true, |_| {});
            },
        );
        output.textures_delta.clear();
        output
    };
    let mut output = draw(vec![]);
    for _ in 0..5 {
        output = draw(vec![]);
    }
    let pos = text_rect(&output, "Pan [P]").center();
    let button = |pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let _ = draw(vec![egui::Event::PointerMoved(pos), button(true)]);
    output = draw(vec![button(false)]);
    assert_eq!(
        output.viewport_output[&egui::ViewportId::ROOT].repaint_delay,
        std::time::Duration::ZERO
    );
    // Service the requested frame without supplying a new input event.
    output = draw(vec![]);
    text_rect(&output, "Measure");
    assert!(!output.shapes.iter().any(
        |s| matches!(&s.shape, egui::epaint::Shape::Text(t) if t.galley.job.text == "Distance")
    ));
    for _ in 0..5 {
        output = draw(vec![]);
    }
    assert_eq!(
        output.viewport_output[&egui::ViewportId::ROOT].repaint_delay,
        std::time::Duration::MAX
    );
    assert_eq!(tool, Tool::Pan);
}
