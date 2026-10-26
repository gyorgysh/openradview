// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
pub(super) fn generate(
    folder: &std::path::Path,
    base: &dicom_object::DefaultDicomObject,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut rgb = base.clone();
    for (tag, n) in [(0x0100, 16u16), (0x0101, 16), (0x0102, 15)] {
        rgb.put(DataElement::new(
            Tag(0x0028, tag),
            VR::US,
            PrimitiveValue::from(n),
        ));
    }
    let pixels: Vec<u16> = (0..64 * 64)
        .flat_map(|i| {
            let (x, y) = (i % 64, i / 64);
            [
                (x * 1040) as u16,
                (y * 1040) as u16,
                if x < 32 { 65535 } else { 8192 },
            ]
        })
        .collect();
    rgb.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OW,
        PrimitiveValue::U16(pixels.into()),
    ));
    for icc in [false, true] {
        if icc {
            let mut profile = moxcms::ColorProfile::new_display_p3();
            profile.profile_class = moxcms::ProfileClass::InputDevice;
            rgb.put(DataElement::new(
                Tag(0x0028, 0x2000),
                VR::OB,
                PrimitiveValue::U8(profile.encode()?.into()),
            ));
        }
        let mut bytes = Vec::new();
        rgb.write_all(&mut bytes)?;
        write_new(
            folder.join(if icc {
                "synthetic-rgb16-icc.dcm"
            } else {
                "synthetic-rgb16.dcm"
            }),
            &bytes,
        )?;
    }
    let mut palette = base.clone();
    palette.put_str(Tag(0x0028, 0x0004), VR::CS, "PALETTE COLOR");
    palette.remove_element(Tag(0x0028, 0x0006));
    palette.put(DataElement::new(
        Tag(0x0028, 0x0002),
        VR::US,
        PrimitiveValue::from(1u16),
    ));
    palette.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OB,
        PrimitiveValue::U8((0..4096).map(|i| (i % 256) as u8).collect()),
    ));
    for (c, values) in [
        (1, (0..256).map(|v| v as u16 * 257).collect::<Vec<_>>()),
        (2, (0..256).map(|v| (255 - v) as u16 * 257).collect()),
        (3, vec![32768; 256]),
    ] {
        palette.put(DataElement::new(
            Tag(0x0028, 0x1100 + c),
            VR::US,
            PrimitiveValue::U16(vec![256, 0, 16].into()),
        ));
        palette.put(DataElement::new(
            Tag(0x0028, 0x1200 + c),
            VR::OW,
            PrimitiveValue::U16(values.into()),
        ));
    }
    let mut bytes = Vec::new();
    palette.write_all(&mut bytes)?;
    write_new(folder.join("synthetic-palette.dcm"), &bytes)?;
    Ok(())
}

pub(super) fn volume(
    folder: &std::path::Path,
    base: &dicom_object::DefaultDicomObject,
) -> Result<(), Box<dyn std::error::Error>> {
    let folder = folder.join("synthetic-color-volume");
    fs::create_dir(&folder)?;
    for z in 0..8 {
        let mut frame = base.clone();
        for (tag, n) in [(0x0100, 16u16), (0x0101, 16), (0x0102, 15)] {
            frame.put(DataElement::new(
                Tag(0x0028, tag),
                VR::US,
                PrimitiveValue::from(n),
            ));
        }
        frame.put_str(
            Tag(0x0020, 0x000e),
            VR::UI,
            "1.2.826.0.1.3680043.10.543.912",
        );
        frame.put_str(Tag(0x0020, 0x0011), VR::IS, "12");
        frame.put_str(Tag(0x0020, 0x0032), VR::DS, format!("0\\0\\{}", z * 4));
        frame.put_str(Tag(0x0020, 0x0013), VR::IS, (z + 1).to_string());
        let pixels: Vec<u16> = (0..64 * 64)
            .flat_map(|i| {
                let (x, y) = (i % 64, i / 64);
                let inside = (x as f64 - 31.5).powi(2)
                    + (y as f64 - 31.5).powi(2)
                    + ((z as f64 - 3.5) * 4.).powi(2)
                    < 24f64.powi(2);
                if inside {
                    [(x * 1040) as u16, (y * 1040) as u16, (z * 9000) as u16]
                } else {
                    [0; 3]
                }
            })
            .collect();
        frame.put(DataElement::new(
            Tag(0x7fe0, 0x0010),
            VR::OW,
            PrimitiveValue::U16(pixels.into()),
        ));
        let mut bytes = Vec::new();
        frame.write_all(&mut bytes)?;
        write_new(folder.join(format!("generated-{z}.dcm")), &bytes)?;
    }
    Ok(())
}
