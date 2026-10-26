// SPDX-License-Identifier: GPL-3.0-or-later
//! Original sparse fixtures: logical GiB, only a few physical filesystem blocks.
use super::*;
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};

pub(super) fn create_sparse_file(path: &Path) -> File {
    let file = File::create(path).unwrap();
    // NTFS needs an explicit sparse flag before multi-GiB extension; other
    // hosts already leave unwritten fixture ranges unallocated.
    #[cfg(windows)]
    {
        let status = std::process::Command::new("fsutil")
            .args(["sparse", "setflag"])
            .arg(path)
            .output()
            .expect("NTFS sparse fixture setup must run");
        assert!(status.status.success(), "NTFS sparse fixture setup failed");
    }
    file
}

pub(super) fn native(folder: &Path) -> (PathBuf, Vec<u8>, u64) {
    native_frames(folder, 8191)
}
pub(super) fn native_frames(folder: &Path, frames: u32) -> (PathBuf, Vec<u8>, u64) {
    native_with_padding(folder, frames, None)
}
pub(super) fn native_with_padding(
    folder: &Path,
    frames: u32,
    padding: Option<i16>,
) -> (PathBuf, Vec<u8>, u64) {
    let mut object =
        dicom_object::from_reader(std::io::Cursor::new(support::fixture(0., 1, false, false)))
            .unwrap();
    object.remove_element(Tag(0x7fe0, 0x0010));
    if let Some(padding) = padding {
        object.put(DataElement::new(
            Tag(0x0028, 0x0120),
            VR::SS,
            PrimitiveValue::from(padding),
        ));
    }
    object.put(DataElement::new(
        Tag(0x0028, 0x0010),
        VR::US,
        PrimitiveValue::from(512u16),
    ));
    object.put(DataElement::new(
        Tag(0x0028, 0x0011),
        VR::US,
        PrimitiveValue::from(512u16),
    ));
    object.put_str(Tag(0x0028, 0x0008), VR::IS, frames.to_string());
    let bytes = 512u64 * 512 * 2 * u64::from(frames);
    let mut header = Vec::new();
    object.write_all(&mut header).unwrap();
    header.extend_from_slice(&[0xe0, 0x7f, 0x10, 0, b'O', b'W', 0, 0]);
    header.extend_from_slice(&(bytes as u32).to_le_bytes());
    let path = folder.join("synthetic-four-gib.dcm");
    let mut file = create_sparse_file(&path);
    file.write_all(&header).unwrap();
    file.set_len(header.len() as u64 + bytes).unwrap();
    patch(&mut file, header.len() as u64, false);
    patch(&mut file, header.len() as u64 + bytes - 512 * 512 * 2, true);
    (path, header, bytes)
}
fn patch(file: &mut File, offset: u64, last: bool) {
    file.seek(SeekFrom::Start(offset)).unwrap();
    for value in if last {
        [100u16, 50, 0, 65535]
    } else {
        [0u16, 50, 100, 65535]
    } {
        file.write_all(&value.to_le_bytes()).unwrap();
    }
}
pub(super) fn zip64(folder: &Path, native: &Path, header: &[u8], bytes: u64) -> PathBuf {
    let mut crc = crc32fast::Hasher::new();
    let mut source = File::open(native).unwrap();
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        let n = source.read(&mut buffer).unwrap();
        if n == 0 {
            break;
        }
        crc.update(&buffer[..n]);
    }
    let crc = crc.finalize();
    let size = header.len() as u64 + bytes;
    let name = b"synthetic.dcm";
    let path = folder.join("synthetic-four-gib.zip");
    let mut file = create_sparse_file(&path);
    let mut local = Vec::new();
    local.extend_from_slice(&0x04034b50u32.to_le_bytes());
    for n in [45u16, 0, 0, 0, 0] {
        local.extend_from_slice(&n.to_le_bytes());
    }
    for n in [crc, u32::MAX, u32::MAX] {
        local.extend_from_slice(&n.to_le_bytes());
    }
    for n in [name.len() as u16, 20] {
        local.extend_from_slice(&n.to_le_bytes());
    }
    local.extend_from_slice(name);
    local.extend_from_slice(&[1, 0, 16, 0]);
    local.extend_from_slice(&size.to_le_bytes());
    local.extend_from_slice(&size.to_le_bytes());
    file.write_all(&local).unwrap();
    file.write_all(header).unwrap();
    let offset = local.len() as u64 + header.len() as u64;
    file.set_len(local.len() as u64 + size).unwrap();
    patch(&mut file, offset, false);
    patch(&mut file, offset + bytes - 512 * 512 * 2, true);
    let central_offset = (1u64 << 32) + 65536;
    file.seek(SeekFrom::Start(central_offset)).unwrap();
    let mut central = Vec::new();
    central.extend_from_slice(&0x02014b50u32.to_le_bytes());
    for n in [45u16, 45, 0, 0, 0, 0] {
        central.extend_from_slice(&n.to_le_bytes());
    }
    for n in [crc, u32::MAX, u32::MAX] {
        central.extend_from_slice(&n.to_le_bytes());
    }
    for n in [name.len() as u16, 20, 0, 0, 0] {
        central.extend_from_slice(&n.to_le_bytes());
    }
    central.extend_from_slice(&0u32.to_le_bytes());
    central.extend_from_slice(&0u32.to_le_bytes());
    central.extend_from_slice(name);
    central.extend_from_slice(&[1, 0, 16, 0]);
    central.extend_from_slice(&size.to_le_bytes());
    central.extend_from_slice(&size.to_le_bytes());
    file.write_all(&central).unwrap();
    let zip64_end = central_offset + central.len() as u64;
    file.write_all(&0x06064b50u32.to_le_bytes()).unwrap();
    file.write_all(&44u64.to_le_bytes()).unwrap();
    file.write_all(&45u16.to_le_bytes()).unwrap();
    file.write_all(&45u16.to_le_bytes()).unwrap();
    for n in [0u32, 0] {
        file.write_all(&n.to_le_bytes()).unwrap();
    }
    for n in [1u64, 1, central.len() as u64, central_offset] {
        file.write_all(&n.to_le_bytes()).unwrap();
    }
    file.write_all(&0x07064b50u32.to_le_bytes()).unwrap();
    file.write_all(&0u32.to_le_bytes()).unwrap();
    file.write_all(&zip64_end.to_le_bytes()).unwrap();
    file.write_all(&1u32.to_le_bytes()).unwrap();
    file.write_all(&0x06054b50u32.to_le_bytes()).unwrap();
    for n in [0u16, 0, u16::MAX, u16::MAX] {
        file.write_all(&n.to_le_bytes()).unwrap();
    }
    file.write_all(&u32::MAX.to_le_bytes()).unwrap();
    file.write_all(&u32::MAX.to_le_bytes()).unwrap();
    file.write_all(&0u16.to_le_bytes()).unwrap();
    path
}
