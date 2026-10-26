// SPDX-License-Identifier: GPL-3.0-or-later
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod annotations;
mod app;
mod brand;
mod controls;
mod copy;
mod demos;
mod dialogs;
mod export;
mod frame_source;
mod frame_worker;
mod help;
mod image_info;
mod image_overlay;
mod labels;
mod mpr;
mod mpr_viewport;
mod mpr_worker;
mod oblique_ui;
mod playback;
#[cfg(feature = "qa")]
mod qa;
mod rendered;
mod series_browser;
#[cfg(feature = "mcp")]
mod sharing;
mod single_view;
mod theme;
mod toolbar;
#[cfg(feature = "updates")]
mod updates_ui;
mod view_context;
mod viewport;
mod volume_ui;
mod worker;
mod workspace;

use eframe::{egui, egui_wgpu};

fn main() -> eframe::Result {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args == ["--version"] {
        println!("OpenRadView {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.first().is_some_and(|a| a == "--update-health-check") {
        let valid = args.len() == 2 && args[1] == env!("CARGO_PKG_VERSION");
        std::process::exit(if valid { 0 } else { 1 });
    }

    #[cfg(feature = "qa")]
    let diagnostics = qa::Recorder::new(std::time::Instant::now());
    let mut setup = egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    setup.instance_descriptor.backends = orv_render::native_backends();
    // An explicit empty icon tells eframe to preserve the native bundle icon.
    // A runtime PNG replaces AppKit's Liquid Glass icon with a flat mobile tile.
    #[cfg(target_os = "macos")]
    let icon = egui::IconData::default();
    #[cfg(not(target_os = "macos"))]
    let icon = image::load_from_memory(include_bytes!("../../../assets/brand/app-icon.png"))
        .map(|img| {
            let rgba = img.into_rgba8();
            egui::IconData {
                width: rgba.width(),
                height: rgba.height(),
                rgba: rgba.into_raw(),
            }
        })
        .unwrap_or_default();
    let viewport = egui::ViewportBuilder::default()
        .with_title(if cfg!(feature = "qa") {
            "OpenRadView QA — Local timing"
        } else {
            "OpenRadView — Personal research viewer"
        })
        .with_inner_size([1280.0, 840.0])
        .with_min_inner_size([860.0, 640.0])
        .with_icon(icon);
    #[cfg(target_os = "macos")]
    let viewport = viewport
        .with_title_shown(false)
        .with_titlebar_shown(false)
        .with_fullsize_content_view(true)
        .with_titlebar_buttons_shown(true);
    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: eframe::WgpuConfiguration {
            surface: eframe::SurfaceConfig::LOW_LATENCY,
            wgpu_setup: egui_wgpu::WgpuSetup::CreateNew(setup),
            ..Default::default()
        },
        centered: true,
        persist_window: false,
        ..Default::default()
    };
    eframe::run_native(
        "OpenRadView",
        options,
        Box::new(move |cc| {
            #[cfg(feature = "qa")]
            let app = app::OpenRadView::new(cc).with_diagnostics(diagnostics);
            #[cfg(not(feature = "qa"))]
            let app = app::OpenRadView::new(cc);
            Ok(Box::new(app))
        }),
    )
}
