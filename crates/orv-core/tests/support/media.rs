// SPDX-License-Identifier: GPL-3.0-or-later
//! Independent byte-level fixture writer; pointers include the Part 10 preamble.
use dicom_object::meta::FileMetaTableBuilder;

pub const STUDY: &str = "2.25.271828";
pub const SERIES: &str = "2.25.314159265358979";
pub const IMAGE_CLASS: &str = "1.2.840.10008.5.1.4.1.1.2";
pub const SYNTAX: &str = "1.2.840.10008.1.2.1";

#[derive(Clone)]
pub struct Record {
    pub kind: &'static str,
    pub next: Option<usize>,
    pub lower: Option<usize>,
    pub active: u16,
    pub file: Option<String>,
    pub instance: String,
}
pub fn records() -> Vec<Record> {
    ["PATIENT", "STUDY", "SERIES", "IMAGE", "IMAGE"]
        .into_iter()
        .enumerate()
        .map(|(i, kind)| Record {
            kind,
            next: (i == 3).then_some(4),
            lower: (i < 3).then_some(i + 1),
            active: u16::MAX,
            file: (i >= 3).then(|| format!("IMAGES\\IMG{}", i - 2)),
            instance: format!("2.25.314159{}", i.saturating_sub(2)),
        })
        .collect()
}
pub fn element(output: &mut Vec<u8>, tag: (u16, u16), vr: &[u8; 2], value: &[u8]) {
    output.extend(tag.0.to_le_bytes());
    output.extend(tag.1.to_le_bytes());
    output.extend(vr);
    output.extend((value.len() as u16).to_le_bytes());
    output.extend(value);
}
fn text(output: &mut Vec<u8>, tag: (u16, u16), vr: &[u8; 2], value: &str) {
    let mut value = value.as_bytes().to_vec();
    if !value.len().is_multiple_of(2) {
        value.push(if vr == b"UI" { 0 } else { b' ' })
    }
    element(output, tag, vr, &value);
}
fn body(record: &Record, offsets: &[u32]) -> Vec<u8> {
    let mut bytes = Vec::new();
    element(
        &mut bytes,
        (4, 0x1400),
        b"UL",
        &record.next.map_or(0, |i| offsets[i]).to_le_bytes(),
    );
    element(&mut bytes, (4, 0x1410), b"US", &record.active.to_le_bytes());
    element(
        &mut bytes,
        (4, 0x1420),
        b"UL",
        &record.lower.map_or(0, |i| offsets[i]).to_le_bytes(),
    );
    text(&mut bytes, (4, 0x1430), b"CS", record.kind);
    if let Some(file) = &record.file {
        text(&mut bytes, (4, 0x1500), b"CS", file);
        text(&mut bytes, (4, 0x1510), b"UI", IMAGE_CLASS);
        text(&mut bytes, (4, 0x1511), b"UI", &record.instance);
        text(&mut bytes, (4, 0x1512), b"UI", SYNTAX);
    }
    match record.kind {
        "PATIENT" => text(&mut bytes, (0x10, 0x10), b"PN", "Generated^DirectoryOnly"),
        "STUDY" => text(&mut bytes, (0x20, 0xd), b"UI", STUDY),
        "SERIES" => text(&mut bytes, (0x20, 0xe), b"UI", SERIES),
        _ => (),
    }
    bytes
}
pub fn directory(records: &[Record], order: &[usize], undefined: bool) -> (Vec<u8>, Vec<u32>) {
    let mut bytes = vec![0u8; 128];
    bytes.extend(b"DICM");
    FileMetaTableBuilder::new()
        .media_storage_sop_class_uid("1.2.840.10008.1.3.10")
        .media_storage_sop_instance_uid("2.25.7777")
        .transfer_syntax(SYNTAX)
        .build()
        .unwrap()
        .write(&mut bytes)
        .unwrap();
    text(&mut bytes, (4, 0x1130), b"CS", "GENERATED");
    let root_at = bytes.len();
    element(&mut bytes, (4, 0x1200), b"UL", &0u32.to_le_bytes());
    element(&mut bytes, (4, 0x1202), b"UL", &0u32.to_le_bytes());
    element(&mut bytes, (4, 0x1212), b"US", &0u16.to_le_bytes());
    bytes.extend([4, 0, 0x20, 0x12, b'S', b'Q', 0, 0]);
    let sequence_length_at = bytes.len();
    bytes.extend(0u32.to_le_bytes());
    let sequence_start = bytes.len();
    let mut offsets = vec![0u32; records.len()];
    let mut position = bytes.len();
    for &i in order {
        offsets[i] = position as u32;
        position += 8 + body(&records[i], &offsets).len() + if undefined { 8 } else { 0 };
    }
    for &i in order {
        let body = body(&records[i], &offsets);
        bytes.extend([0xfe, 0xff, 0, 0xe0]);
        bytes.extend(
            if undefined {
                u32::MAX
            } else {
                body.len() as u32
            }
            .to_le_bytes(),
        );
        bytes.extend(body);
        if undefined {
            bytes.extend([0xfe, 0xff, 0x0d, 0xe0, 0, 0, 0, 0])
        }
    }
    let length = if undefined {
        u32::MAX
    } else {
        (bytes.len() - sequence_start) as u32
    };
    bytes[sequence_length_at..sequence_length_at + 4].copy_from_slice(&length.to_le_bytes());
    if undefined {
        bytes.extend([0xfe, 0xff, 0xdd, 0xe0, 0, 0, 0, 0])
    }
    if !records.is_empty() {
        bytes[root_at + 8..root_at + 12].copy_from_slice(&offsets[0].to_le_bytes());
        bytes[root_at + 20..root_at + 24].copy_from_slice(&offsets[0].to_le_bytes());
    }
    (bytes, offsets)
}
