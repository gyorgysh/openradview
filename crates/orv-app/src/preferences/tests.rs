// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::actions::Action;
#[test]
fn bounded_settings_roundtrip_only_bindings_and_cancel_preserves_previous_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("controls.json");
    assert_eq!(load(&path).unwrap(), Bindings::default());
    assert!(!path.exists(), "Opening the app does not create settings");
    let mut bindings = Bindings::default();
    bindings.set(Action::Pan, Some('B')).unwrap();
    save(&path, bindings, &|| false).unwrap();
    let bytes = fs::read(&path).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json.as_object().unwrap().len(), 2);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["bindings"].as_object().unwrap().len(), 1);
    assert_eq!(json["bindings"]["keys"].as_array().unwrap().len(), 13);
    assert_eq!(load(&path).unwrap(), bindings);
    assert!(save(&path, Bindings::default(), &|| true).is_err());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
#[test]
fn invalid_unknown_oversized_and_nonregular_settings_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("controls.json");
    let mut json = serde_json::to_value(File {
        schema_version: 1,
        bindings: Bindings::default(),
    })
    .unwrap();
    json["bindings"]["keys"][0] = "Z".into();
    fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
    assert!(load(&path).is_err());
    json["bindings"]["keys"][0] = "p".into();
    fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
    assert!(load(&path).is_err());
    json["bindings"]["keys"][0] = "P".into();
    json["study_path"] = "forbidden".into();
    fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
    assert!(load(&path).is_err());
    fs::write(&path, vec![b' '; 8193]).unwrap();
    assert!(load(&path).is_err());
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(load(&path).is_err());
    #[cfg(unix)]
    {
        fs::remove_dir(&path).unwrap();
        std::os::unix::fs::symlink(dir.path().join("missing"), &path).unwrap();
        assert!(load(&path).is_err());
    }
}

#[test]
fn saving_moves_focus_to_close_and_keeps_it_when_apply_becomes_disabled() {
    fn echo(request: Request, _: &dyn Fn() -> bool) -> Result<Bindings, String> {
        Ok(match request {
            Request::Save(bindings) => bindings,
            Request::Load => Bindings::default(),
        })
    }
    fn render(
        ctx: &egui::Context,
        settings: &mut Preferences,
        events: Vec<egui::Event>,
    ) -> egui::accesskit::TreeUpdate {
        settings.poll(ctx);
        let mut output = ctx.run_ui(
            egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280., 840.),
                )),
                ..Default::default()
            },
            |_| settings.show(ctx),
        );
        output.textures_delta.clear();
        output.platform_output.accesskit_update.take().unwrap()
    }
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    crate::theme::install(&ctx);
    let mut settings = Preferences::new();
    settings.started = true;
    settings.loaded = true;
    settings.open = true;
    settings.query = "Pan".into();
    settings.draft.set(Action::Pan, Some('B')).unwrap();
    settings.worker =
        Some(crate::jobs::Worker::start(ctx.clone(), "test-control-focus", echo).unwrap());
    let tree = render(&ctx, &mut settings, vec![]);
    let node = |name: &str| {
        tree.nodes
            .iter()
            .find(|(_, node)| {
                node.role() == egui::accesskit::Role::Button && node.label() == Some(name)
            })
            .map(|(id, _)| *id)
            .unwrap()
    };
    let apply = node("Apply shortcuts");
    let close = node("Close shortcuts");
    render(
        &ctx,
        &mut settings,
        vec![egui::Event::AccessKitActionRequest(
            egui::accesskit::ActionRequest {
                action: egui::accesskit::Action::Focus,
                target_tree: egui::accesskit::TreeId::ROOT,
                target_node: apply,
                data: None,
            },
        )],
    );
    let tree = render(
        &ctx,
        &mut settings,
        vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    assert_eq!(tree.focus, close);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut tree = render(&ctx, &mut settings, vec![]);
    while settings.pending.is_some() {
        assert!(std::time::Instant::now() < deadline);
        tree = render(&ctx, &mut settings, vec![]);
        std::thread::yield_now();
    }
    assert_eq!(settings.active.letter(Action::Pan), Some('B'));
    assert_eq!(tree.focus, close);
    assert!(
        tree.nodes
            .iter()
            .find(|(id, _)| *id == apply)
            .unwrap()
            .1
            .is_disabled()
    );
    assert_eq!(
        tree.nodes
            .iter()
            .find(|(_, node)| node.value() == Some("Shortcuts saved on this device"))
            .unwrap()
            .1
            .live(),
        Some(egui::accesskit::Live::Polite)
    );
}
