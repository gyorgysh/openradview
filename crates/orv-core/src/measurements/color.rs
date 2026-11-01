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
    color_roi_shape_stats_cancellable(frame, RoiShape::Rectangle(corners), cancelled)
        .map(|stats| stats.samples)
}
#[derive(Clone, Debug, PartialEq)]
pub struct ColorRegionStats {
    pub samples: ColorRoiStats,
    pub geometric_area_mm2: Option<f64>,
}
pub fn color_roi_shape_stats(
    frame: &Frame,
    shape: RoiShape<'_>,
) -> Result<ColorRegionStats, &'static str> {
    color_roi_shape_stats_cancellable(frame, shape, || false)
}
/// Worker-only RGB code reduction; source precision and ICC domain remain separate.
pub fn color_roi_shape_stats_cancellable(
    frame: &Frame,
    region: RoiShape<'_>,
    cancelled: impl Fn() -> bool,
) -> Result<ColorRegionStats, &'static str> {
    if cancelled() {
        return Err("Cancelled");
    }
    if frame.slab.is_some() {
        return Err("Slab projections are not physical measurement planes");
    }
    let rgba = frame
        .rgba
        .as_ref()
        .ok_or("RGB statistics need a color image")?;
    let count = (frame.width as usize)
        .checked_mul(frame.height as usize)
        .filter(|n| *n > 0)
        .ok_or("ROI coordinates or image are invalid")?;
    if !frame.pixels.is_empty()
        || count.checked_mul(4) != Some(rgba.len())
        || frame
            .validity
            .as_ref()
            .is_some_and(|mask| mask.len() != count)
        || frame.color.as_ref().is_some_and(|data| {
            count.checked_mul(3) != Some(data.rgb.len()) || !(1..=16).contains(&data.info.bits)
        })
    {
        return Err("ROI coordinates or image are invalid");
    }
    let region = region.prepare([frame.width, frame.height], &cancelled)?;
    let maximum = frame.color.as_ref().map_or(255, |data| data.info.maximum());
    let mut moments: [reduction::Moments; 3] = std::array::from_fn(|_| Default::default());
    region.visit(frame.width, &cancelled, |index| {
        if !frame.has_sample(index) {
            return Ok(());
        }
        for (channel, moment) in moments.iter_mut().enumerate() {
            let value = frame
                .color
                .as_ref()
                .map_or(u16::from(rgba[index * 4 + channel]), |data| {
                    data.rgb[index * 3 + channel]
                });
            if value > maximum {
                return Err("RGB code values exceed the declared precision");
            }
            moment.add(f64::from(value));
        }
        Ok(())
    })?;
    let count = moments[0].count;
    if count == 0 {
        return Err("The ROI contains no color samples");
    }
    Ok(ColorRegionStats {
        samples: ColorRoiStats {
            count,
            channels: moments.map(|m| ChannelStats {
                min: m.min,
                max: m.max,
                mean: m.mean,
                standard_deviation: m.standard_deviation(),
            }),
            bits: frame.color.as_ref().map_or(8, |data| data.info.bits),
            domain: if frame.color.is_some() {
                ColorRoiDomain::BeforeIcc
            } else {
                ColorRoiDomain::SrgbPresentation
            },
            area_mm2: shape::area_mm2(frame, count as f64),
        },
        geometric_area_mm2: shape::area_mm2(frame, region.area_pixels),
    })
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
