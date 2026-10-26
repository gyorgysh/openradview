// SPDX-License-Identifier: GPL-3.0-or-later
use eframe::{
    egui,
    egui_wgpu::{self, wgpu},
};
use orv_core::{
    ViewState,
    volume_view::{Quality, VolumeView},
};
use orv_render::volume::{VolumeRenderer, VolumeTarget};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone, PartialEq)]
struct Key {
    display: ViewState,
    state: VolumeView,
    interactive: [u32; 2],
    refined: [u32; 2],
}
pub(super) struct Gpu {
    pub renderer: VolumeRenderer,
    key: Option<Key>,
    idle_after: Instant,
    row: u32,
    front: usize,
    pub error: Option<&'static str>,
    pub progress: f32,
    pub refined: bool,
}
impl Gpu {
    pub fn preferred_snapshot_size(&self) -> Option<[u32; 2]> {
        let key = self.key.as_ref()?;
        let size = if self.refined {
            key.refined
        } else {
            key.interactive
        };
        let scale = (512. / size[0].max(size[1]) as f32).min(1.);
        Some(size.map(|v| (v as f32 * scale).round().max(1.) as u32))
    }
    pub fn completed(&self, display: &ViewState, state: &VolumeView) -> bool {
        self.error.is_none()
            && self
                .key
                .as_ref()
                .is_some_and(|key| key.display == *display && key.state == *state)
            && (state.quality == Quality::Interactive || self.refined)
    }
    #[cfg(all(test, feature = "gpu-tests"))]
    pub(super) fn expire_idle(&mut self) {
        self.idle_after = Instant::now();
    }
    pub fn new(renderer: VolumeRenderer) -> Self {
        Self {
            renderer,
            key: None,
            idle_after: Instant::now(),
            row: 0,
            front: 0,
            error: None,
            progress: 0.,
            refined: false,
        }
    }
}
pub(super) struct Callback {
    pub gpu: Arc<Mutex<Gpu>>,
    pub display: ViewState,
    pub state: VolumeView,
    pub pixels: egui::Vec2,
    pub interacting: bool,
    pub ctx: egui::Context,
}
fn resolution(pixels: egui::Vec2, cap: f32) -> [u32; 2] {
    let scale = (cap / pixels.x.max(pixels.y).max(1.)).min(1.);
    [
        (pixels.x * scale).round().clamp(1., 1024.) as u32,
        (pixels.y * scale).round().clamp(1., 1024.) as u32,
    ]
}
impl egui_wgpu::CallbackTrait for Callback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        _resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Ok(mut gpu) = self.gpu.try_lock() else {
            self.ctx.request_repaint();
            return vec![];
        };
        let key = Key {
            display: self.display.clone(),
            state: self.state.clone(),
            interactive: resolution(self.pixels, 256.),
            refined: resolution(self.pixels, 768.),
        };
        let result = (|| {
            if gpu.key.as_ref() != Some(&key) {
                let mut state = self.state.clone();
                state.quality = Quality::Interactive;
                gpu.renderer.render_target(
                    device,
                    encoder,
                    &self.display,
                    &state,
                    VolumeTarget {
                        slot: 0,
                        size: key.interactive,
                        region: [[0, 0], key.interactive],
                    },
                )?;
                gpu.front = 0;
                gpu.refined = false;
                gpu.progress = 0.;
                gpu.row = 0;
                gpu.idle_after = Instant::now() + Duration::from_millis(180);
                gpu.key = Some(key.clone());
                self.ctx.request_repaint();
            }
            if self.interacting {
                gpu.idle_after = Instant::now() + Duration::from_millis(180);
            }
            if self.state.quality == Quality::Interactive {
                if gpu.progress != 1. {
                    gpu.progress = 1.;
                    self.ctx.request_repaint();
                }
                return Ok(());
            }
            let now = Instant::now();
            if now < gpu.idle_after {
                self.ctx.request_repaint_after(gpu.idle_after - now);
                return Ok(());
            }
            if gpu.row < key.refined[1] {
                let mut state = self.state.clone();
                state.quality = Quality::Refined;
                let height = (key.refined[1] - gpu.row).min(8);
                let row = gpu.row;
                gpu.renderer.render_target(
                    device,
                    encoder,
                    &self.display,
                    &state,
                    VolumeTarget {
                        slot: 1,
                        size: key.refined,
                        region: [[0, row], [key.refined[0], height]],
                    },
                )?;
                gpu.row += height;
                gpu.progress = gpu.row as f32 / key.refined[1] as f32;
                if gpu.row == key.refined[1] {
                    gpu.front = 1;
                    gpu.refined = true;
                }
                self.ctx.request_repaint();
            }
            Ok::<_, &'static str>(())
        })();
        let previous = gpu.error;
        gpu.error = result.err();
        if previous != gpu.error {
            self.ctx.request_repaint();
        }
        vec![]
    }
    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        _resources: &egui_wgpu::CallbackResources,
    ) {
        if let Ok(gpu) = self.gpu.try_lock()
            && gpu.error.is_none()
        {
            gpu.renderer.paint_target(gpu.front, pass);
        }
    }
}
