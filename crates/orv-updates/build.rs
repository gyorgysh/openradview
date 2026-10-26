// SPDX-License-Identifier: GPL-3.0-or-later
fn main() {
    if let Ok(target) = std::env::var("TARGET") {
        println!("cargo:rustc-env=ORV_UPDATE_TARGET={target}");
    }
}
