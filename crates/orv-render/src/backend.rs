// SPDX-License-Identifier: GPL-3.0-or-later
//! Native backend selection shared by the window and offscreen renderer.
use wgpu::{Backend, Backends, DownlevelFlags, Limits};

pub fn configured_backends() -> Result<Backends, &'static str> {
    let requested = std::env::var("WGPU_BACKEND");
    match requested {
        Ok(value) => select(Some(&value), crate::native_backends()),
        Err(std::env::VarError::NotPresent) => Ok(crate::native_backends()),
        Err(_) => Err("WGPU_BACKEND must contain native backend names. See openradview --help."),
    }
}

fn select(requested: Option<&str>, supported: Backends) -> Result<Backends, &'static str> {
    let Some(requested) = requested else {
        return Ok(supported);
    };
    let mut selected = Backends::empty();
    for name in requested.split(',') {
        let backend = match name.trim().to_ascii_lowercase().as_str() {
            "vk" | "vulkan" => Backends::VULKAN,
            "dx12" | "d3d12" => Backends::DX12,
            "metal" | "mtl" => Backends::METAL,
            "gl" | "gles" | "opengl" => Backends::GL,
            _ => {
                return Err(
                    "Unknown WGPU_BACKEND. See openradview --help for native backend names.",
                );
            }
        };
        selected |= backend & supported;
    }
    if selected.is_empty() {
        return Err(
            "The requested WGPU_BACKEND is unavailable on this platform. See openradview --help.",
        );
    }
    Ok(selected)
}

pub fn device_descriptor(adapter: &wgpu::Adapter) -> wgpu::DeviceDescriptor<'static> {
    wgpu::DeviceDescriptor {
        label: Some("OpenRadView graphics"),
        required_limits: limits(
            adapter.get_info().backend,
            &adapter.limits(),
            adapter.get_downlevel_capabilities().flags,
        ),
        ..Default::default()
    }
}

fn limits(backend: Backend, available: &Limits, flags: DownlevelFlags) -> Limits {
    let mut required = if backend == Backend::Gl {
        Limits::downlevel_webgl2_defaults()
    } else {
        Limits::default()
    };
    required.max_texture_dimension_2d = available.max_texture_dimension_2d.min(8192);
    if backend == Backend::Gl
        && flags.contains(DownlevelFlags::COMPUTE_SHADERS)
        && crate::volume::check_limits(available).is_ok()
    {
        // Keep slice display available on GL 3.x, while requesting only the
        // compute resources used by ray marching on capable GL/ES drivers.
        required.max_compute_workgroup_size_x = 8;
        required.max_compute_workgroup_size_y = 8;
        required.max_compute_workgroup_size_z = 1;
        required.max_compute_invocations_per_workgroup = 64;
        required.max_compute_workgroups_per_dimension =
            available.max_compute_workgroups_per_dimension.min(65535);
        required.max_storage_textures_per_shader_stage = 1;
        required.max_texture_dimension_3d = available.max_texture_dimension_3d.min(2048);
    }
    required
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_gl_is_honored_without_enabling_web_or_foreign_backends() {
        let linux = Backends::VULKAN | Backends::GL;
        assert_eq!(select(None, linux), Ok(linux));
        for name in ["gl", "GLES", " OpenGL "] {
            assert_eq!(select(Some(name), linux), Ok(Backends::GL));
        }
        assert_eq!(select(Some("gl, vk"), linux), Ok(linux));
        for name in ["", "webgpu", "noop", "mistake", "metal"] {
            assert!(select(Some(name), linux).is_err());
        }
        assert!(select(Some("gl"), Backends::METAL).is_err());
        assert_eq!(
            select(Some("metal,gl"), Backends::METAL),
            Ok(Backends::METAL)
        );
    }

    #[test]
    fn downlevel_display_keeps_compute_disabled_until_the_driver_can_support_it() {
        let available = Limits::default();
        let basic = limits(Backend::Gl, &available, DownlevelFlags::empty());
        assert_eq!(
            basic.max_texture_dimension_2d,
            available.max_texture_dimension_2d.min(8192)
        );
        assert!(crate::volume::check_limits(&basic).is_err());
        let compute = limits(Backend::Gl, &available, DownlevelFlags::COMPUTE_SHADERS);
        assert!(crate::volume::check_limits(&compute).is_ok());
        assert!(compute.check_limits(&available));
        let small = Limits {
            max_texture_dimension_2d: 4096,
            ..available
        };
        assert_eq!(
            limits(Backend::Gl, &small, DownlevelFlags::empty()).max_texture_dimension_2d,
            4096
        );
    }
}
