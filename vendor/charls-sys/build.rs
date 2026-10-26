// SPDX-License-Identifier: MIT
// OpenRadView: rebuild when the pinned native source changes.
fn main() {
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=charls");

    #[cfg(not(feature = "vcpkg"))]
    {
        let mut config = cmake::Config::new("charls");
        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
            // Rust uses the non-debug CRT in both profiles. CMake's default
            // Debug runtime would otherwise add MSVCRTD to a static Rust link.
            let static_crt = std::env::var("CARGO_CFG_TARGET_FEATURE")
                .unwrap_or_default()
                .split(',')
                .any(|feature| feature == "crt-static");
            config.static_crt(static_crt).define(
                "CMAKE_MSVC_RUNTIME_LIBRARY",
                if static_crt {
                    "MultiThreaded"
                } else {
                    "MultiThreadedDLL"
                },
            );
        }
        let dst = config
            .define("BUILD_SHARED_LIBS", "0")
            .define("CMAKE_LINK_DEPENDS_USE_LINKER", "0")
            .define("CHARLS_BUILD_TESTS", "0")
            .define("CHARLS_BUILD_FUZZ_TEST", "0")
            .define("CHARLS_BUILD_SAMPLES", "0")
            .always_configure(true)
            .build();

        println!("cargo:rustc-link-search=native={}/lib", dst.display());
        println!(
            "cargo:rustc-link-lib={}=charls",
            if cfg!(feature = "static") {
                "static"
            } else {
                "dylib"
            }
        );
    }

    #[cfg(feature = "vcpkg")]
    vcpkg::Config::new()
        .emit_includes(true)
        .find_package("charls")
        .unwrap();
}
