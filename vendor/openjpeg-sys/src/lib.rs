#![allow(non_camel_case_types, non_snake_case)]

mod ffi;

pub use crate::ffi::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poke() {
        unsafe {
            let tmp = opj_create_compress(CODEC_FORMAT::OPJ_CODEC_J2K);
            assert!(!tmp.is_null());
            opj_destroy_codec(tmp);
        }
    }
}
