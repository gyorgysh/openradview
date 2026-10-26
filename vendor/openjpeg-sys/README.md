# [OpenJPEG][1] (JPEG 2000) bindings for Rust

[![openjpeg-sys on crates.io](https://img.shields.io/crates/v/openjpeg-sys.svg)](https://crates.io/crates/openjpeg-sys)

This is a bare FFI interface for the C library. Builds from source and links statically.

For a high-level API, consider using [jpeg2k][2] instead.

[1]: https://github.com/uclouvain/openjpeg
[2]: https://crates.io/crates/jpeg2k

## Contributing

This crate is passively maintained.
Please file an issue or send a pull request if an important change is needed
or the bindings are outdated.

To update the native OpenJPEG library,
checkout the new revision in the `vendor` git submodule,
edit the header files in [config](./config) according to the new version,
and run [`generate.sh`](./generate.sh) to update the FFI bindings.
