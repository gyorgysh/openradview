// SPDX-License-Identifier: GPL-3.0-or-later
use eframe::{
    Error,
    egui_wgpu::{self, wgpu},
};
use std::sync::Arc;

pub fn setup() -> Result<egui_wgpu::WgpuSetupCreateNew, &'static str> {
    let mut setup = egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    setup.instance_descriptor.backends = orv_render::configured_backends()?;
    setup.device_descriptor = Arc::new(orv_render::device_descriptor);
    Ok(setup)
}

pub fn log_adapter(adapter: &wgpu::Adapter) {
    let info = adapter.get_info();
    eprintln!(
        "OpenRadView graphics: backend={:?}; adapter={}; type={:?}; driver={} {}",
        info.backend,
        single_line(&info.name),
        info.device_type,
        single_line(&info.driver),
        single_line(&info.driver_info)
    );
    if info.device_type == wgpu::DeviceType::Cpu {
        eprintln!(
            "OpenRadView is using software graphics. Large windows and 3D views may be slow. Check that your GPU driver is installed, or try a smaller window."
        );
    }
}

fn single_line(value: &str) -> String {
    value
        .chars()
        .take(200)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

pub fn report(error: &Error) {
    let explanation = match error {
        Error::Wgpu(_) => graphics_help(),
        Error::Winit(_) | Error::WinitEventLoop(_) => {
            "OpenRadView could not open a desktop window. Run it from a working desktop session. On Linux, check your Wayland/X11 display and session settings."
        }
        _ => {
            "OpenRadView could not start. The technical details below may help when reporting the problem."
        }
    };
    eprintln!("{explanation}\n\nTechnical details: {error}");
    // stderr remains useful when the desktop portal is missing or unavailable.
    if cfg!(not(target_os = "linux"))
        || std::env::var_os("DISPLAY").is_some()
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
    {
        rfd::MessageDialog::new()
            .set_title("OpenRadView could not start")
            .set_description(explanation)
            .set_level(rfd::MessageLevel::Error)
            .show();
    }
}

fn graphics_help() -> &'static str {
    if cfg!(target_os = "linux") {
        "OpenRadView could not initialize graphics. A Vulkan or OpenGL driver is required.\n\nOn Debian/Ubuntu, a Mesa Vulkan driver can be installed with:\n  sudo apt install mesa-vulkan-drivers\n\nFor OpenGL, install an EGL/OpenGL driver (Mesa: libegl-mesa0 and libgl1-mesa-dri), then try:\n  WGPU_BACKEND=gl openradview\n\nIf you use a proprietary GPU driver, check its Vulkan/EGL support instead."
    } else {
        "OpenRadView could not initialize graphics. Update the graphics driver and try again. macOS requires Metal; Windows supports DX12 or Vulkan."
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn adapter_diagnostics_cannot_inject_terminal_controls_or_unbounded_lines() {
        assert_eq!(super::single_line("GPU\n\x1b[31m\tname"), "GPU  [31m name");
        assert_eq!(super::single_line(&"x".repeat(1000)).len(), 200);
    }
}
