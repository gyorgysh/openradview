// SPDX-License-Identifier: GPL-3.0-or-later
use std::ffi::OsString;

pub enum Action {
    Open,
    Help,
    Version,
    Health(bool),
}

pub fn parse(args: &[OsString]) -> Result<Action, &'static str> {
    match args {
        [] => Ok(Action::Open),
        [flag] if flag == "--help" || flag == "-h" => Ok(Action::Help),
        [flag] if flag == "--version" || flag == "-V" => Ok(Action::Version),
        [flag, version] if flag == "--update-health-check" => {
            Ok(Action::Health(version == env!("CARGO_PKG_VERSION")))
        }
        [flag, ..] if flag == "--update-health-check" => Ok(Action::Health(false)),
        _ => Err("Unknown option or argument. Run openradview --help for usage."),
    }
}

pub fn help() -> String {
    format!(
        "OpenRadView {}\n\nUsage: openradview [OPTIONS]\n\n  -h, --help       Print this help without opening a window\n  -V, --version    Print the version\n\nOpen files, folders and ZIP archives from the viewer or by drag and drop.\nFor personal education; not for diagnosis or decisions about care.\n\nGraphics:\n  Linux:   Vulkan, with native OpenGL as a fallback\n  Windows: DX12 or Vulkan\n  macOS:   Metal (Apple Silicon packages)\n\nSet WGPU_BACKEND to select a backend, e.g. WGPU_BACKEND=gl openradview\non Linux. Accepted names: vulkan, gl (opengl/gles), dx12, metal.\nThe backend must be supported on your platform.\nSelected adapter and driver details are printed locally to stderr.\n",
        env!("CARGO_PKG_VERSION")
    )
}
