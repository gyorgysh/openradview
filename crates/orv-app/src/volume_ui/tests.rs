// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use eframe::egui_wgpu::{self, CallbackTrait, wgpu};
use orv_core::study::demo_study;
use std::time::{Duration, Instant};

fn device() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: orv_render::configured_backends().expect("Native backend selection"),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .expect("Real GPU required");
    pollster::block_on(adapter.request_device(&orv_render::device_descriptor(&adapter))).unwrap()
}

#[test]
fn workbench_upload_reuses_completed_mpr_source_and_cpu_fallback_keeps_exact_owner() {
    use orv_render::{reslice::GpuReslicer, volume_source::GpuVolumeSource};
    let (device, queue) = device();
    let ctx = egui::Context::default();
    let mut frames = demo_study().series.remove(0).frames;
    frames.truncate(2);
    let volume = Arc::new(Volume::new(frames).unwrap());
    let source = GpuVolumeSource::new(&device, &queue, volume.clone(), || false).unwrap();
    let weak = Arc::downgrade(&source);
    let mut reslicer = GpuReslicer::new(source.clone()).unwrap();
    let grids = crate::mpr_worker::PLANES
        .map(|plane| volume.reformat_grid(plane, volume.center(), 64).unwrap());
    let planes = reslicer
        .render_frames(grids.each_ref().map(Some), || false)
        .unwrap();
    for plane in planes.iter().flatten() {
        assert!(Arc::ptr_eq(plane.source(), &source));
    }
    let mut shared = Upload::from_source(
        planes[0].as_ref().unwrap().source().clone(),
        wgpu::TextureFormat::Rgba8Unorm,
        ctx.clone(),
    );
    let ready = shared
        .receiver
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap();
    assert!(
        Arc::ptr_eq(&ready.source, &source),
        "3D must retain the MPR upload, not recreate it"
    );
    assert!(Arc::ptr_eq(&ready.volume, &volume));
    shared.worker.take().unwrap().join().unwrap();
    let mut session = VolumeSession::new(true);
    session.upload = Some(Upload::from_volume(
        volume.clone(),
        device.clone(),
        queue.clone(),
        wgpu::TextureFormat::Rgba8Unorm,
        ctx.clone(),
    ));
    let handle = session.upload.as_mut().unwrap().worker.take().unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while session.gpu.is_none() && session.error.is_none() {
        session.prepare_test_frame(&device, &queue, &ctx);
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(session.error.is_none(), "{:?}", session.error);
    assert!(Arc::ptr_eq(session.volume.as_ref().unwrap(), &volume));
    handle.join().unwrap();
    drop((ready, shared, planes, reslicer, source, session));
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(30)),
        })
        .unwrap();
    assert!(
        weak.upgrade().is_none(),
        "Closed workbench retained its GPU source"
    );
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
                material: None,
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
    let mut upload = Upload::start(
        frames,
        device.clone(),
        queue,
        wgpu::TextureFormat::Rgba8Unorm,
        ctx,
    );
    let worker = upload.worker.take().unwrap();
    drop(upload);
    let deadline = Instant::now() + Duration::from_secs(3);
    while weak.upgrade().is_some() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(weak.upgrade().is_none());
    // Source release can precede device cleanup on the worker. The test process
    // must not exit while its last native graphics owner is still unwinding.
    while !worker.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        worker.is_finished(),
        "Cancelled upload did not finish cleanup"
    );
    worker.join().unwrap();
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(30)),
        })
        .unwrap();
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
        material: None,
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

#[test]
fn native_material_uses_shared_source_and_never_presents_a_partial_refinement() {
    use orv_render::{material_volume::MaterialVolumeRenderer, volume_source::GpuVolumeSource};
    let (device, queue) = device();
    let ctx = egui::Context::default();
    let volume = Arc::new(Volume::new(demo_study().series[0].frames.clone()).unwrap());
    let source = GpuVolumeSource::new(&device, &queue, volume.clone(), || false).unwrap();
    let renderer = orv_render::volume::VolumeRenderer::from_source(
        source.clone(),
        wgpu::TextureFormat::Rgba8Unorm,
        || false,
    )
    .unwrap();
    let material_renderer =
        MaterialVolumeRenderer::new(source, wgpu::TextureFormat::Rgba8Unorm).unwrap();
    let mut engine = Gpu::new(renderer);
    engine.material_renderer = Some(material_renderer);
    let gpu = Arc::new(Mutex::new(engine));
    let editor = super::material::MaterialEditor::new(&TransferPreset::Bone.values()).unwrap();
    let mut state = VolumeView::fit(&volume);
    state.projection = Projection::Composite;
    let mut callback = Callback {
        gpu: gpu.clone(),
        display: ViewState::default(),
        state,
        pixels: Vec2::new(2200., 1400.),
        interacting: false,
        material: Some(editor.style()),
        ctx: ctx.clone(),
    };
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [2200, 1400],
        pixels_per_point: 1.,
    };
    let mut encoder = device.create_command_encoder(&Default::default());
    callback.prepare(
        &device,
        &queue,
        &screen,
        &mut encoder,
        &mut Default::default(),
    );
    queue.submit([encoder.finish()]);
    {
        let engine = gpu.lock().unwrap();
        let output = engine
            .material_renderer
            .as_ref()
            .unwrap()
            .output_texture(0)
            .unwrap();
        assert_eq!([output.width(), output.height()], [512, 326]);
        assert_eq!(output.format(), wgpu::TextureFormat::Rgba16Float);
        assert!(!engine.completed(&callback.display, &callback.state));
        assert!(!engine.refined);
        assert!(engine.error.is_none());
    }
    gpu.lock().unwrap().expire_idle();
    let mut encoder = device.create_command_encoder(&Default::default());
    callback.prepare(
        &device,
        &queue,
        &screen,
        &mut encoder,
        &mut Default::default(),
    );
    queue.submit([encoder.finish()]);
    {
        let engine = gpu.lock().unwrap();
        assert!(!engine.refined);
        assert!((engine.progress - 16. / 1303.).abs() < 0.00001);
        let output = engine
            .material_renderer
            .as_ref()
            .unwrap()
            .output_texture(1)
            .unwrap();
        assert_eq!([output.width(), output.height()], [2048, 1303]);
    }
    callback.material.as_mut().unwrap().lighting = orv_render::material_volume::Lighting::UNLIT;
    let mut encoder = device.create_command_encoder(&Default::default());
    callback.prepare(
        &device,
        &queue,
        &screen,
        &mut encoder,
        &mut Default::default(),
    );
    queue.submit([encoder.finish()]);
    {
        let engine = gpu.lock().unwrap();
        assert_eq!(engine.progress, 0.);
        assert!(!engine.refined);
        assert!(engine.error.is_none());
    }
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(30)),
        })
        .unwrap();
}
