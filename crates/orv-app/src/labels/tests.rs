// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use orv_core::{
    ViewState, WindowLevel,
    study::{Frame, Geometry},
    volume::Volume,
};
fn labels() -> Arc<Labels> {
    let frames = (0..3)
        .map(|z| {
            Arc::new(Frame {
                scalar: None,
                validity: None,
                color: None,
                nifti: None,
                rgba: None,
                width: 8,
                height: 8,
                pixels: vec![20.; 64].into(),
                geometry: Some(Geometry {
                    nifti_transform: None,
                    frame_of_reference: Some("synthetic-brush-grid".into()),
                    position: [0., 0., z as f64],
                    orientation: [1., 0., 0., 0., 1., 0.],
                    spacing: [1., 1.],
                }),
                instance: z,
                frame_index: 0,
                monochrome1: false,
                default_window: WindowLevel {
                    function: Default::default(),
                    width: 100.,
                    center: 50.,
                },
            })
        })
        .collect();
    Arc::new(Labels::empty(Arc::new(Volume::new(frames).unwrap())).unwrap())
}
#[test]
fn primary_click_paints_on_source_grid_and_keeps_undo_checkpoint() {
    let ctx = egui::Context::default();
    let current = labels();
    let image = current.source().frames()[1].clone();
    let texture = ctx.load_texture(
        "test-overlay",
        egui::ColorImage::filled([8, 8], egui::Color32::TRANSPARENT),
        egui::TextureOptions::NEAREST,
    );
    let mut state = LabelUi {
        current: Some(current.clone()),
        paint: true,
        texture: Some((1, texture)),
        ..Default::default()
    };
    let pos = egui::pos2(200., 200.);
    for pressed in [None, Some(true), Some(false)] {
        let mut events = vec![egui::Event::PointerMoved(pos)];
        if let Some(pressed) = pressed {
            events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(500., 500.),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                let (_, response) =
                    ui.allocate_exact_size(egui::vec2(400., 400.), egui::Sense::click_and_drag());
                state.canvas(ui, &response, &image, &ViewState::default(), 0);
            },
        );
        output.textures_delta.clear();
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while state.busy() && std::time::Instant::now() < deadline {
        state.poll();
        std::thread::yield_now();
    }
    assert!(!state.busy(), "brush worker did not finish");
    assert!(state.dirty);
    assert_eq!(state.undo.len(), 1);
    assert!(current.values().iter().all(|v| *v == 0));
    assert!(state.current.as_ref().unwrap().values()[64..128].contains(&1));
    assert!(
        state.current.as_ref().unwrap().values()[..64]
            .iter()
            .all(|v| *v == 0)
    );
    assert!(Arc::ptr_eq(state.undo.front().unwrap(), &current));
}
#[test]
fn cancelled_edit_does_not_change_the_approved_source_or_labels() {
    let current = labels();
    let flag = std::sync::atomic::AtomicBool::new(true);
    let result = worker::run(
        Operation::Threshold(current.clone(), None, [0., 100.], 2),
        &flag,
    );
    assert!(result.is_err() || matches!(result, Ok(Output::Cancelled)));
    assert!(current.values().iter().all(|v| *v == 0));
    assert_eq!(current.source().frames()[0].pixels[0], 20.);
}
