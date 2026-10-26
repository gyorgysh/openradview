// Required for C++ link flags to be honored:
// https://github.com/dtolnay/link-cplusplus/blob/1c5fad6df6b234808a33da0ae3f26fec4f55199f/README.md?plain=1#L54-L62
extern crate link_cplusplus;

mod bindings;
pub use bindings::*;
