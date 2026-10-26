// SPDX-License-Identifier: GPL-3.0-or-later
//! Real, isolated loopback requests; no production transport code in the UI.
use super::*;
use orv_mcp::{broker::RequestBroker, consent::BearerToken, http::ServerStatus};
use std::io::{Read, Write};

fn request(port: u16, key: &str, method: &str) -> serde_json::Value {
    let mut stream = std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_secs(2),
    )
    .unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let body = if method == "POST" {
        r#"{"recipient":"Synthetic dialog test"}"#
    } else {
        ""
    };
    write!(stream, "{method} /request HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {key}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    let mut response = String::new();
    stream.take(8193).read_to_string(&mut response).unwrap();
    assert!(response.len() <= 8192);
    let (header, body) = response.split_once("\r\n\r\n").unwrap();
    assert!(header.starts_with("HTTP/1.1 200 "));
    serde_json::from_str(body).unwrap()
}

#[test]
fn inbound_approval_waits_for_local_dialogs_then_escape_denies_without_a_grant() {
    // External covers Help/Privacy; the rest are independent workspace dialogs.
    for dialog in ["external", "information", "labels", "discard", "export"] {
        let ctx = egui::Context::default();
        let mut workspace = workspace();
        let broker = RequestBroker::start_on(0, Arc::new(|| {})).unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        let port = loop {
            match broker.status() {
                ServerStatus::Listening { port } => break port,
                ServerStatus::Starting if Instant::now() < until => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                other => panic!("Isolated broker did not start: {other:?}"),
            }
        };
        workspace.sharing.broker = Some(broker);
        workspace.sharing.broker_attempted = true;
        match dialog {
            "external" => (),
            "information" => workspace.image_info.open(
                workspace.current_image().unwrap().1,
                24,
                workspace.view.clone(),
                ctx.clone(),
            ),
            "labels" => workspace.labels.open = true,
            "discard" => workspace.labels.discard_question = true,
            "export" => {
                use eframe::egui_wgpu::wgpu;
                let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                    backends: orv_render::native_backends(),
                    ..wgpu::InstanceDescriptor::new_without_display_handle()
                });
                let adapter =
                    pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
                let (device, queue) =
                    pollster::block_on(adapter.request_device(&Default::default())).unwrap();
                let series = Arc::new(workspace.study.as_ref().unwrap().series[0].clone());
                let spec = workspace.rendered_view().unwrap().0;
                let prepared = PreparedSeries::new(0, series.clone(), || false).unwrap();
                workspace.export.open(crate::export::Input {
                    scene: spec.scene(prepared.description(), [64, 64]),
                    series,
                    pixels: crate::export::Pixels::Image(workspace.current_image().unwrap().1),
                    device,
                    queue,
                });
            }
            _ => unreachable!(),
        }
        let capability = BearerToken::generate().unwrap();
        let key = capability.expose_for_configuration();
        let pending = serde_json::json!({"status":"pending"});
        assert_eq!(request(port, key, "POST"), pending);
        for _ in 0..3 {
            workspace.sharing_before_ui(&ctx, dialog == "external");
            assert!(
                workspace.sharing.draft.is_none(),
                "{dialog} was interrupted"
            );
            assert!(!workspace.sharing.is_active());
            assert_eq!(request(port, key, "GET"), pending);
        }
        workspace.image_info.close();
        workspace.export.close();
        workspace.labels.open = false;
        workspace.labels.discard_question = false;
        workspace.sharing_before_ui(&ctx, false);
        let draft = workspace
            .sharing
            .draft
            .as_ref()
            .expect("Deferred request must appear after closing");
        assert_eq!(draft.recipient, "Synthetic dialog test");
        assert!(draft.request_id.is_some());
        assert!(!workspace.sharing.is_active());
        assert_eq!(request(port, key, "GET"), pending);
        // Paint once so egui knows the top modal, then exercise its real close path.
        let mut first = ctx.run_ui(Default::default(), |ui| workspace.sharing.ui(ui.ctx()));
        first.textures_delta.clear();
        let mut close = ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                }],
                ..Default::default()
            },
            |ui| workspace.sharing.ui(ui.ctx()),
        );
        close.textures_delta.clear();
        assert!(workspace.sharing.draft.is_none());
        assert!(!workspace.sharing.is_active());
        assert_eq!(
            request(port, key, "GET"),
            serde_json::json!({"status":"denied"})
        );
        workspace.close();
    }
}
