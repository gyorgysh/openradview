// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::dicom_acquisition_phantom as phantom;
use eframe::egui;
use orv_core::acquisition::{CineMetadata, DicomCatalog};

fn metadata(object: &dicom_object::DefaultDicomObject) -> Arc<CineTiming> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cine.dcm");
    object.write_to_file(&path).unwrap();
    let study = orv_core::index::index_paths(&[path], || false, |_| {}).unwrap();
    let catalog = DicomCatalog::prepare(&study, || false).unwrap();
    let CineMetadata::Available(timing) = catalog.cine(0).unwrap() else {
        panic!("Known timing");
    };
    timing.clone()
}
fn source(variable: bool) -> Playback {
    let mut player = Playback::default();
    player.bind_source(Some(metadata(&phantom::cine(variable))));
    assert!(player.set_source_mode(true));
    player
}
fn ms(value: u64) -> Duration {
    Duration::from_millis(value)
}

#[test]
fn manual_slow_decode_receives_full_interval_and_never_skips() {
    let now = Instant::now();
    let mut player = Playback::default();
    assert_eq!(player.start_at(0, 100), Some(0));
    assert_eq!(player.tick(now, 0, Some(0), 100), Step::Wait(ms(125)));
    assert_eq!(
        player.tick(now + ms(5000), 0, Some(0), 100),
        Step::Advance(1)
    );
    assert_eq!(player.tick(now + ms(20000), 1, Some(0), 100), Step::Idle);
    assert!(player.running);
    let ready = now + ms(30000);
    assert_eq!(player.tick(ready, 1, Some(1), 100), Step::Wait(ms(125)));
    assert_eq!(
        player.tick(ready + ms(124), 1, Some(1), 100),
        Step::Wait(ms(1))
    );
    assert_eq!(
        player.tick(ready + ms(125), 1, Some(1), 100),
        Step::Advance(2)
    );
}
#[test]
fn manual_end_restart_loop_and_pause_keep_their_browsing_policy() {
    let now = Instant::now();
    let mut player = Playback::default();
    assert_eq!(player.start_at(2, 3), Some(0));
    assert_eq!(player.start_at(1, 3), Some(1));
    assert_eq!(player.tick(now, 1, Some(1), 3), Step::Wait(ms(125)));
    assert_eq!(player.tick(now + ms(125), 1, Some(1), 3), Step::Advance(2));
    assert_eq!(
        player.tick(now + ms(150), 2, Some(2), 3),
        Step::Wait(ms(125))
    );
    assert_eq!(player.tick(now + ms(275), 2, Some(2), 3), Step::Idle);
    assert!(!player.running);
    player.repeat = true;
    player.start_at(1, 3);
    player.tick(now, 1, Some(1), 3);
    player.tick(now + ms(125), 1, Some(1), 3);
    player.tick(now + ms(150), 2, Some(2), 3);
    assert_eq!(player.tick(now + ms(275), 2, Some(2), 3), Step::Advance(0));
    player.pause();
    assert_eq!(player.tick(now + ms(500), 0, Some(0), 3), Step::Idle);
    assert_eq!(player.start_at(0, 1), None);
    assert!(!player.running);
}
#[test]
fn fixed_source_trim_waits_for_each_exact_image_records_pace_and_stops_without_a_final_dwell() {
    let now = Instant::now();
    let mut player = source(false);
    player.fps = 1;
    player.repeat = true;
    assert_eq!(player.start_at(0, 5), Some(1));
    assert_eq!(player.tick(now, 1, Some(0), 5), Step::Idle);
    let ready = now + ms(1000);
    assert_eq!(player.tick(ready, 1, Some(1), 5), Step::Wait(ms(50)));
    assert_eq!(
        player.tick(ready + ms(49), 1, Some(1), 5),
        Step::Wait(ms(1))
    );
    assert_eq!(player.tick(ready + ms(50), 1, Some(1), 5), Step::Advance(2));
    assert_eq!(player.tick(ready + ms(240), 2, Some(1), 5), Step::Idle);
    assert_eq!(
        player.tick(ready + ms(250), 2, Some(2), 5),
        Step::Wait(ms(50))
    );
    assert_eq!(
        player.pace,
        Some(Pace {
            elapsed: ms(250),
            interval: ms(50),
            image_wait: ms(200)
        })
    );
    assert_eq!(
        player.tick(ready + ms(300), 2, Some(2), 5),
        Step::Advance(3)
    );
    assert_eq!(player.tick(ready + ms(900), 3, Some(2), 5), Step::Idle);
    assert!(
        player.running,
        "End is not reached until its exact image is ready"
    );
    assert_eq!(player.tick(ready + ms(1000), 3, Some(3), 5), Step::Idle);
    assert!(!player.running);
    assert_eq!(player.notice, Some("Reached the source trim end."));
    assert_eq!(player.tick(ready + ms(10000), 3, Some(3), 5), Step::Idle);
    assert_eq!(player.start_at(3, 5), Some(1));
    assert_eq!(player.start_at(4, 5), Some(1));
}
#[test]
fn variable_source_uses_the_interval_before_the_next_frame_without_delay_wrap_or_catchup() {
    let now = Instant::now();
    let mut player = source(true);
    assert_eq!(player.start_at(1, 5), Some(1));
    // Vector [0,40,120,60,80] ms, trim 2..4: dwells are 120 then 60.
    assert_eq!(player.tick(now, 1, Some(1), 5), Step::Wait(ms(120)));
    assert_eq!(player.tick(now + ms(119), 1, Some(1), 5), Step::Wait(ms(1)));
    assert_eq!(
        player.tick(now + ms(10000), 1, Some(1), 5),
        Step::Advance(2)
    );
    assert_eq!(
        player.tick(now + ms(10001), 2, Some(2), 5),
        Step::Wait(ms(60))
    );
    assert_eq!(
        player.tick(now + ms(10061), 2, Some(2), 5),
        Step::Advance(3)
    );
    assert_eq!(player.tick(now + ms(10062), 3, Some(3), 5), Step::Idle);
    assert!(!player.running);
    assert_eq!(player.start_at(2, 5), Some(2));
    assert_eq!(player.tick(now, 2, Some(2), 5), Step::Wait(ms(60)));
    player.pause();
    assert_eq!(player.tick(now + ms(10000), 2, Some(2), 5), Step::Idle);
    assert!(player.shown.is_none() && player.requested.is_none());
    assert_eq!(player.start_at(2, 5), Some(2));
    assert_eq!(
        player.tick(now + ms(10001), 2, Some(2), 5),
        Step::Wait(ms(60))
    );
}
#[test]
fn mode_changes_and_source_capabilities_pause_and_drop_pending_state_without_changing_manual_settings()
 {
    let now = Instant::now();
    let mut player = source(false);
    player.fps = 17;
    player.repeat = true;
    let same = player.source.as_ref().unwrap().clone();
    let old = Arc::downgrade(&same);
    player.start_at(1, 5);
    player.tick(now, 1, Some(1), 5);
    assert!(!player.bind_source(Some(same)));
    assert!(player.running);
    assert!(player.set_source_mode(false));
    assert!(!player.running);
    assert_eq!((player.fps, player.repeat), (17, true));
    player.start_at(1, 5);
    assert_eq!(
        player.tick(now, 1, Some(1), 5),
        Step::Wait(Duration::from_secs_f64(1. / 17.))
    );
    assert!(player.set_source_mode(true));
    player.start_at(1, 5);
    player.tick(now, 1, Some(1), 5);
    player.tick(now + ms(50), 1, Some(1), 5);
    assert!(player.requested.is_some());
    assert!(player.bind_source(Some(metadata(&phantom::cine(false)))));
    assert!(old.upgrade().is_none(), "Old timing capability retained");
    assert!(!player.running && !player.source_mode && player.requested.is_none());
    assert!(!player.set_source_mode(false));
    player.bind_source(None);
    assert!(!player.set_source_mode(true));
    assert!(player.source.is_none());
}
#[test]
fn context_changes_invalid_ranges_and_unrepresentable_deadlines_stop_without_a_timer() {
    let now = Instant::now();
    let mut player = source(false);
    assert_eq!(player.start_at(1, 4), None);
    assert_eq!(player.start_at(5, 5), None);
    player.start_at(1, 5);
    assert_eq!(player.tick(now, 2, Some(2), 5), Step::Idle);
    assert!(!player.running);
    player.start_at(1, 5);
    assert_eq!(player.tick(now, 1, Some(1), 6), Step::Idle);
    assert!(!player.running);
    let mut object = phantom::cine(false);
    object.put_str(dicom_core::Tag(0x0018, 0x1063), dicom_core::VR::DS, "4e21");
    // Fits a Duration and the entire 5-frame table. Find the host clock's
    // representable edge without assuming its epoch or platform-specific range.
    player.bind_source(Some(metadata(&object)));
    player.set_source_mode(true);
    player.start_at(1, 5);
    let near_limit = (0..64).rev().fold(now, |time, shift| {
        time.checked_add(Duration::from_secs(1u64 << shift))
            .unwrap_or(time)
    });
    assert!(
        near_limit
            .checked_add(Duration::from_secs_f64(4e18))
            .is_none()
    );
    assert_eq!(player.tick(near_limit, 1, Some(1), 5), Step::Idle);
    assert!(!player.running);
    assert!(player.notice.unwrap().contains("monotonic clock range"));
    assert_eq!(player.tick(now, 1, Some(1), 5), Step::Idle);
}

fn render(
    player: &mut Playback,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    allowed: bool,
) -> (egui::accesskit::TreeUpdate, Duration) {
    let metadata = player
        .source
        .as_ref()
        .map(|timing| CineMetadata::Available(timing.clone()));
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(600., 220.),
            )),
            events,
            ..Default::default()
        },
        |ui| {
            let mut target = 1;
            ui.horizontal_wrapped(|ui| player.buttons(ui, &mut target, 5, allowed));
            if let Some(enabled) = player.source_controls(ui, metadata.as_ref(), allowed) {
                player.set_source_mode(enabled);
            }
        },
    );
    output.textures_delta.clear();
    (
        output.platform_output.accesskit_update.take().unwrap(),
        output.viewport_output[&egui::ViewportId::ROOT].repaint_delay,
    )
}
fn action(tree: &egui::accesskit::TreeUpdate, label: &str) -> egui::Event {
    let target = tree
        .nodes
        .iter()
        .find_map(|(id, n)| (n.label() == Some(label)).then_some(*id))
        .unwrap();
    egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest {
        action: egui::accesskit::Action::Click,
        target_tree: egui::accesskit::TreeId::ROOT,
        target_node: target,
        data: None,
    })
}
#[test]
fn source_controls_are_explicit_disable_manual_inputs_and_settle_without_an_idle_timer() {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    crate::theme::install(&ctx);
    let mut player = source(false);
    player.set_source_mode(false);
    let (tree, _) = render(&mut player, &ctx, vec![], true);
    let click = action(&tree, "Source intervals");
    render(&mut player, &ctx, vec![click], false);
    assert!(!player.source_mode, "Disabled controls changed mode");
    let (tree, _) = render(&mut player, &ctx, vec![], true);
    let click = action(&tree, "Source intervals");
    render(&mut player, &ctx, vec![click], true);
    assert!(player.source_mode);
    let (tree, _) = render(&mut player, &ctx, vec![], true);
    for label in ["Loop", "Manual playback rate"] {
        assert!(
            tree.nodes
                .iter()
                .any(|(_, n)| n.label() == Some(label) && n.is_disabled()),
            "{label} was active"
        );
    }
    let text = tree
        .nodes
        .iter()
        .filter_map(|(_, n)| n.label().or(n.value()))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Images 2–4") && text.contains("stops at end"));
    let mut delay = Duration::ZERO;
    for _ in 0..6 {
        delay = render(&mut player, &ctx, vec![], true).1;
    }
    assert!(
        delay > Duration::from_secs(1),
        "Paused controls kept a repaint timer"
    );
}
#[test]
fn single_image_trim_is_inspectable_but_cannot_be_enabled_as_source_playback() {
    let mut object = phantom::cine(false);
    object.put_str(dicom_core::Tag(0x0008, 0x2143), dicom_core::VR::IS, "2");
    let mut player = Playback::default();
    player.bind_source(Some(metadata(&object)));
    assert!(!player.set_source_mode(true));
    assert_eq!(
        player.start_at(1, 5),
        Some(1),
        "Manual browsing remains available"
    );
}

#[test]
fn unavailable_timing_explains_the_refusal_without_relabelling_manual_playback() {
    use orv_core::acquisition::CineError;
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let player = Playback::default();
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        assert!(
            player
                .source_controls(ui, Some(&CineMetadata::Unavailable(CineError::Order)), true)
                .is_none()
        );
    });
    output.textures_delta.clear();
    let tree = output.platform_output.accesskit_update.unwrap();
    assert!(
        tree.nodes
            .iter()
            .any(|(_, n)| n.label() == Some("Source intervals") && n.is_disabled())
    );
    assert!(tree.nodes.iter().any(|(_, n)| {
        n.value()
            .or(n.label())
            .is_some_and(|text| text.contains("reordered or mixed-object"))
    }));
    assert!(!player.source_mode && !player.running);
}
