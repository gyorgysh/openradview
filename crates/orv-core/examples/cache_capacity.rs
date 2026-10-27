// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit generated-input cache benchmark; no patient files or installed-app logging.
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_object::{InMemDicomObject, meta::FileMetaTableBuilder};
use orv_core::{
    index::index_paths,
    resources::{MemoryBudget, memory_policy},
};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    sync::Arc,
    time::Instant,
};

struct Input(std::path::PathBuf);
impl Drop for Input {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let count = 600u32;
    let side = 512u16;
    let object = InMemDicomObject::from_element_iter([
        DataElement::new(Tag(0x0008, 0x0016), VR::UI, "1.2.840.10008.5.1.4.1.1.7"),
        DataElement::new(Tag(0x0008, 0x0018), VR::UI, "2.25.600512"),
        DataElement::new(Tag(0x0020, 0x000e), VR::UI, "2.25.6005121"),
        DataElement::new(Tag(0x0028, 0x0010), VR::US, PrimitiveValue::from(side)),
        DataElement::new(Tag(0x0028, 0x0011), VR::US, PrimitiveValue::from(side)),
        DataElement::new(Tag(0x0028, 0x0008), VR::IS, count.to_string()),
        DataElement::new(Tag(0x0028, 0x0002), VR::US, PrimitiveValue::from(1u16)),
        DataElement::new(Tag(0x0028, 0x0004), VR::CS, "MONOCHROME2"),
        DataElement::new(Tag(0x0028, 0x0100), VR::US, PrimitiveValue::from(16u16)),
        DataElement::new(Tag(0x0028, 0x0101), VR::US, PrimitiveValue::from(16u16)),
        DataElement::new(Tag(0x0028, 0x0102), VR::US, PrimitiveValue::from(15u16)),
        DataElement::new(Tag(0x0028, 0x0103), VR::US, PrimitiveValue::from(0u16)),
        DataElement::new(
            Tag(0x7fe0, 0x0010),
            VR::OW,
            PrimitiveValue::U16(Vec::new().into()),
        ),
    ])
    .with_meta(FileMetaTableBuilder::new().transfer_syntax("1.2.840.10008.1.2.1"))?;
    let mut header = Vec::new();
    object.write_all(&mut header)?;
    let payload = count * u32::from(side) * u32::from(side) * 2;
    let length = header.len();
    header[length - 4..].copy_from_slice(&payload.to_le_bytes());
    let path = std::env::temp_dir().join(format!("orv-generated-cache-{}.dcm", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let input = Input(path);
    file.write_all(&header)?;
    file.set_len(header.len() as u64 + u64::from(payload))?;
    drop(file);
    let indexing = Instant::now();
    let study = index_paths(std::slice::from_ref(&input.0), || false, |_| {})?;
    let index_ms = indexing.elapsed().as_secs_f64() * 1000.;
    let policy = memory_policy();
    let series = &study.series[0];
    let mut identities = Vec::new();
    let mut first_decode_ms = 0.;
    let cold = Instant::now();
    for index in 0..count {
        let started = Instant::now();
        let image = series.read_frame(index as usize, || false)?;
        if index == 0 {
            first_decode_ms = started.elapsed().as_secs_f64() * 1000.;
        }
        if image.pixels.len() != usize::from(side).pow(2)
            || image.pixels.iter().any(|value| *value != 0.)
        {
            return Err("Generated pixel check failed".into());
        }
        identities.push(Arc::downgrade(&image));
    }
    let cold_ms = cold.elapsed().as_secs_f64() * 1000.;
    let retained = MemoryBudget::decoded_cache().used();
    let mut samples = Vec::new();
    let mut hits = 0;
    for index in (0..count).rev() {
        let start = Instant::now();
        let image = series.read_frame(index as usize, || false)?;
        samples.push(start.elapsed().as_secs_f64() * 1000.);
        if identities[index as usize]
            .upgrade()
            .is_some_and(|cached| Arc::ptr_eq(&cached, &image))
        {
            hits += 1;
        }
    }
    samples.sort_by(f64::total_cmp);
    drop(study);
    if identities.iter().any(|image| image.upgrade().is_some())
        || MemoryBudget::decoded_cache().used() != 0
    {
        return Err("Closed source retained cached pixels".into());
    }
    println!(
        "{{\"decoded_bytes\":{},\"cache_limit_bytes\":{},\"volume_limit_bytes\":{},\"retained_bytes\":{},\"index_ms\":{index_ms:.4},\"first_decode_ms\":{first_decode_ms:.4},\"cold_total_ms\":{cold_ms:.4},\"reverse_cache_hits\":{hits},\"frame_count\":{count},\"warm_median_ms\":{:.4},\"warm_p95_ms\":{:.4},\"released\":true}}",
        u64::from(count) * u64::from(side).pow(2) * 4,
        policy.cache_bytes,
        policy.volume_bytes,
        retained,
        samples[samples.len() / 2],
        samples[samples.len() * 95 / 100]
    );
    Ok(())
}
