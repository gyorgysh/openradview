// SPDX-License-Identifier: GPL-3.0-or-later
//! Per-channel code statistics; never interpreted as scalar modality measurements.
use super::*;
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorRoiDomain {
    BeforeIcc,
    SrgbPresentation,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChannelStats {
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub standard_deviation: f64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColorRoiStats {
    pub count: usize,
    pub channels: [ChannelStats; 3],
    pub bits: u16,
    pub domain: ColorRoiDomain,
    pub area_mm2: Option<f64>,
}
pub fn color_roi_stats(
    frame: &Frame,
    corners: [[f64; 2]; 2],
) -> Result<ColorRoiStats, &'static str> {
    color_roi_stats_cancellable(frame, corners, || false)
}
pub fn color_roi_stats_cancellable(
    frame: &Frame,
    corners: [[f64; 2]; 2],
    cancelled: impl Fn() -> bool,
) -> Result<ColorRoiStats, &'static str> {
    if cancelled() {
        return Err("Cancelled");
    }
    let rgba = frame
        .rgba
        .as_ref()
        .ok_or("RGB statistics need a color image")?;
    let count = u64::from(frame.width) * u64::from(frame.height);
    if count == 0
        || !frame.pixels.is_empty()
        || Some(rgba.len() as u64) != count.checked_mul(4)
        || frame.color.as_ref().is_some_and(|data| {
            Some(data.rgb.len() as u64) != count.checked_mul(3)
                || !(1..=16).contains(&data.info.bits)
        })
        || corners.iter().flatten().any(|v| !v.is_finite())
    {
        return Err("ROI coordinates or image are invalid");
    }
    let min: [f64; 2] = std::array::from_fn(|i| corners[0][i].min(corners[1][i]));
    let max: [f64; 2] = std::array::from_fn(|i| corners[0][i].max(corners[1][i]));
    if min[0] < 0.
        || min[1] < 0.
        || max[0] > (frame.width - 1) as f64
        || max[1] > (frame.height - 1) as f64
    {
        return Err("ROI must lie inside the image");
    }
    let mut stats = ColorRoiStats {
        count: 0,
        channels: std::array::from_fn(|_| ChannelStats {
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
            mean: 0.,
            standard_deviation: 0.,
        }),
        bits: frame.color.as_ref().map_or(8, |data| data.info.bits),
        domain: if frame.color.is_some() {
            ColorRoiDomain::BeforeIcc
        } else {
            ColorRoiDomain::SrgbPresentation
        },
        area_mm2: None,
    };
    let mut m2 = [0.; 3];
    for y in min[1].ceil() as u32..=max[1].floor() as u32 {
        if cancelled() {
            return Err("Cancelled");
        }
        for x in min[0].ceil() as u32..=max[0].floor() as u32 {
            let index = (y * frame.width + x) as usize;
            if !frame.has_sample(index) {
                continue;
            }
            stats.count += 1;
            for (c, m2) in m2.iter_mut().enumerate() {
                let value = frame
                    .color
                    .as_ref()
                    .map_or(rgba[index * 4 + c] as f64, |data| {
                        data.rgb[index * 3 + c] as f64
                    });
                let s = &mut stats.channels[c];
                if value > frame.color.as_ref().map_or(255, |data| data.info.maximum()) as f64 {
                    return Err("RGB code values exceed the declared precision");
                }
                let delta = value - s.mean;
                s.mean += delta / stats.count as f64;
                *m2 += delta * (value - s.mean);
                s.min = s.min.min(value);
                s.max = s.max.max(value);
            }
        }
    }
    if stats.count == 0 {
        return Err("The ROI contains no color samples");
    }
    for (s, m2) in stats.channels.iter_mut().zip(m2) {
        s.standard_deviation = (m2 / stats.count as f64).max(0.).sqrt();
    }
    stats.area_mm2 = frame
        .geometry
        .as_ref()
        .filter(|g| g.validate().is_ok())
        .map(|g| stats.count as f64 * g.spacing[0] * g.spacing[1]);
    Ok(stats)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::{ColorData, ColorEncoding, ColorInfo};
    use std::sync::Arc;
    #[test]
    fn channel_statistics_preserve_codes_exclude_missing_pixels_and_require_color() {
        let mut frame = (*crate::study::demo_study().series[0].frames[0]).clone();
        frame.width = 2;
        frame.height = 2;
        frame.pixels = Arc::from([]);
        frame.rgba = Some(Arc::from([
            0, 0, 0, 255, 255, 255, 255, 255, 128, 128, 128, 255, 0, 0, 0, 0,
        ]));
        frame.color = Some(Arc::new(ColorData {
            info: ColorInfo {
                source_encoding: ColorEncoding::Rgb,
                bits: 16,
                icc_sha256: None,
            },
            rgb: Arc::from([
                0, 100, 200, 65535, 101, 201, 32768, 102, 202, 60000, 65535, 65535,
            ]),
        }));
        let stats = color_roi_stats(&frame, [[0., 0.], [1., 1.]]).unwrap();
        assert_eq!(stats.count, 3);
        assert_eq!(stats.bits, 16);
        assert_eq!(stats.area_mm2, Some(3.));
        assert!((stats.channels[0].mean - 98303. / 3.).abs() < 1e-8);
        assert_eq!(stats.channels[1].mean, 101.);
        assert!((stats.channels[1].standard_deviation - (2f64 / 3.).sqrt()).abs() < 1e-8);
        assert_eq!(stats.domain, ColorRoiDomain::BeforeIcc);
        assert!(roi_stats(&frame, [[0., 0.], [1., 1.]]).is_err());
        assert!(color_roi_stats(&frame, [[1., 1.], [1., 1.]]).is_err());
        assert!(color_roi_stats(&frame, [[-1., 0.], [1., 1.]]).is_err());
        frame.color = None;
        assert_eq!(
            color_roi_stats(&frame, [[0., 0.], [1., 1.]])
                .unwrap()
                .domain,
            ColorRoiDomain::SrgbPresentation
        );
    }
}
