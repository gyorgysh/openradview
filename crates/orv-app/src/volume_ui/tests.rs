// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use eframe::egui_wgpu::{self, CallbackTrait, wgpu};
use orv_core::study::demo_study;
use std::time::{Duration, Instant};

fn device() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: orv_render::native_backends(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .expect("Real GPU required");
    pollster::block_on(adapter.request_device(&Default::default())).unwrap()
}

impl VolumeSession {
    pub(crate) fn prepare_test_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        ctx: &egui::Context,
    ) {
        self.poll();
        if let (Some(gpu), Some(state)) = (&self.gpu, &self.state) {
            gpu.lock().unwrap().expire_idle();
            let callback = Callback {
                gpu: gpu.clone(),
                display: self.display.clone(),
                state: state.clone(),
                pixels: Vec2::new(96., 64.),
                interacting: false,
                ctx: ctx.clone(),
            };
            let mut encoder = device.create_command_encoder(&Default::default());
            callback.prepare(
                device,
                queue,
                &egui_wgpu::ScreenDescriptor {
                    size_in_pixels: [96, 64],
                    pixels_per_point: 1.,
                },
                &mut encoder,
                &mut Default::default(),
            );
            queue.submit([encoder.finish()]);
        }
    }
}

#[test]
fn background_upload_cancel_releases_source_without_waiting_on_ui() {
    let (device, queue) = device();
    let ctx = egui::Context::default();
    let study = demo_study();
    let mut frames = study.series[0].frames.clone();
    frames.truncate(2);
    let weak = Arc::downgrade(&frames[0]);
    drop(study);
    let upload = Upload::start(frames, device, queue, wgpu::TextureFormat::Rgba8Unorm, ctx);
    drop(upload);
    let deadline = Instant::now() + Duration::from_secs(3);
    while weak.upgrade().is_some() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(weak.upgrade().is_none());
}

#[test]
fn idle_refinement_is_tiled_and_new_input_replaces_the_pending_view() {
    let (device, queue) = device();
    let ctx = egui::Context::default();
    let study = demo_study();
    let volume = Arc::new(Volume::new(study.series[0].frames.clone()).unwrap());
    let renderer = orv_render::volume::VolumeRenderer::new(
        &device,
        &queue,
        volume.clone(),
        wgpu::TextureFormat::Rgba8Unorm,
        || false,
    )
    .unwrap();
    let gpu = Arc::new(Mutex::new(Gpu::new(renderer)));
    let mut callback = Callback {
        gpu: gpu.clone(),
        display: ViewState::default(),
        state: VolumeView::fit(&volume),
        pixels: Vec2::new(96., 64.),
        interacting: false,
        ctx,
    };
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [96, 64],
        pixels_per_point: 1.,
    };
    let prepare = |callback: &Callback| {
        let mut resources = egui_wgpu::CallbackResources::default();
        let mut encoder = device.create_command_encoder(&Default::default());
        callback.prepare(&device, &queue, &screen, &mut encoder, &mut resources);
        queue.submit([encoder.finish()]);
    };
    prepare(&callback);
    assert!(!gpu.lock().unwrap().refined);
    gpu.lock().unwrap().expire_idle();
    prepare(&callback);
    assert_eq!(gpu.lock().unwrap().progress, 0.125);
    callback.display.invert = true;
    callback.interacting = true;
    prepare(&callback);
    assert_eq!(gpu.lock().unwrap().progress, 0.0);
    assert!(!gpu.lock().unwrap().refined);
    callback.interacting = false;
    gpu.lock().unwrap().expire_idle();
    for _ in 0..8 {
        prepare(&callback);
    }
    let gpu = gpu.lock().unwrap();
    assert!(gpu.refined);
    assert_eq!(gpu.progress, 1.0);
}

#[test]
fn explicit_preview_quality_stops_without_unrequested_refinement() {
    let (device, queue) = device();
    let ctx = egui::Context::default();
    let study = demo_study();
    let volume = Arc::new(Volume::new(study.series[0].frames.clone()).unwrap());
    let mut session = VolumeSession::start(study.series[0].frames.clone());
    session.state = Some(VolumeView::fit(&volume));
    session.state.as_mut().unwrap().quality = orv_core::volume_view::Quality::Interactive;
    session.needs_fit = false;
    session.start_upload(
        device.clone(),
        queue.clone(),
        wgpu::TextureFormat::Rgba8Unorm,
        ctx.clone(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        session.prepare_test_frame(&device, &queue, &ctx);
        if session.gpu.as_ref().is_some_and(|gpu| {
            gpu.lock()
                .unwrap()
                .completed(&session.display, session.state.as_ref().unwrap())
        }) {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    let gpu = session.gpu.as_ref().unwrap().lock().unwrap();
    assert!(!gpu.refined);
    assert_eq!(gpu.progress, 1.);
}

#[test]
fn non_ct_volume_starts_with_its_source_range_instead_of_bone_thresholds() {
    let (device, queue) = device();
    let ctx = egui::Context::default();
    for modality in ["NIFTI", "MR", "CT"] {
        let mut series = demo_study().series.remove(0);
        series.frames.truncate(2);
        series.modality = modality;
        let mut session = VolumeSession::start(Arc::new(series));
        session.start_upload(
            device.clone(),
            queue.clone(),
            wgpu::TextureFormat::Rgba8Unorm,
            ctx.clone(),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while session.gpu.is_none() && session.error.is_none() && Instant::now() < deadline {
            session.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(session.error.is_none(), "{:?}", session.error);
        assert!(session.gpu.is_some());
        let expected = if modality == "CT" {
            TransferPreset::Bone.values()
        } else {
            session.source_range.clone().unwrap()
        };
        assert_eq!(session.display.window, expected.0);
        assert_eq!(session.state.as_ref().unwrap().transfer, expected.1);
        assert_eq!(session.initial_display, expected);
    }
}
