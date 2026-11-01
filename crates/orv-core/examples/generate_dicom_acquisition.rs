// SPDX-License-Identifier: GPL-3.0-or-later
//! Original numeric enhanced and cine inputs; no anatomy or patient attributes.
#[path = "../tests/support/dicom_axes.rs"]
mod phantom;
use std::{fs, io::Write, path::PathBuf};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let folder = PathBuf::from(
        args.next()
            .ok_or("Pass a fresh DICOM acquisition fixture directory")?,
    );
    if args.next().is_some() {
        return Err("Pass only the output directory".into());
    }
    fs::create_dir_all(&folder)?;
    let mut invalid = phantom::cine(true);
    invalid.put_str(dicom_core::Tag(0x0018, 0x1065), dicom_core::VR::DS, "0\\40");
    let mut unscoped = phantom::enhanced();
    unscoped.remove_element(dicom_core::Tag(0x0020, 0x9221));
    for (index, (name, mut object)) in [
        ("enhanced-axes.dcm", phantom::enhanced()),
        ("fixed-cine.dcm", phantom::cine(false)),
        ("variable-cine.dcm", phantom::cine(true)),
        ("invalid-timing.dcm", invalid),
        ("unscoped-axes.dcm", unscoped),
    ]
    .into_iter()
    .enumerate()
    {
        let uid = format!("2.25.515100{}", index + 1);
        object.put_str(dicom_core::Tag(0x0008, 0x0018), dicom_core::VR::UI, &uid);
        object.update_meta(|meta| meta.media_storage_sop_instance_uid = uid);
        let mut bytes = Vec::new();
        object.write_all(&mut bytes)?;
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(folder.join(name))?
            .write_all(&bytes)?;
    }
    println!(
        "Wrote original numeric enhanced-axis and cine fixtures, including explicit refusal cases; no patient data."
    );
    Ok(())
}
