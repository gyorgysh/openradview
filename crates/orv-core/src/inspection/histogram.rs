// SPDX-License-Identifier: GPL-3.0-or-later
use crate::study::Frame;

#[derive(Clone, Debug, PartialEq)]
pub struct ChannelHistogram {
    pub min: f64,
    pub max: f64,
    /// Equal-width bins. Last bin includes the maximum; constant data uses bin0.
    pub bins: Vec<u64>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Histogram {
    pub channels: Vec<ChannelHistogram>,
    pub count: u64,
    pub missing: u64,
    pub domain: &'static str,
}

/// Full-frame counts in source scalar values or pre-ICC RGB codes. Display
/// window/inversion does not affect this distribution. No voxel sampling stride.
pub fn histogram(
    frame: &Frame,
    bins: usize,
    cancelled: impl Fn() -> bool,
) -> Result<Histogram, &'static str> {
    let count = u64::from(frame.width) * u64::from(frame.height);
    let channels = if frame.rgba.is_some() { 3 } else { 1 };
    if count == 0
        || count > (32 * 1024 * 1024) / channels
        || !(64..=4096).contains(&bins)
        || frame
            .rgba
            .as_ref()
            .is_some_and(|v| v.len() as u64 != count * 4 || !frame.pixels.is_empty())
        || (channels == 1 && frame.pixels.len() as u64 != count)
        || frame.color.as_ref().is_some_and(|v| {
            channels != 3 || v.rgb.len() as u64 != count * 3 || !(1..=16).contains(&v.info.bits)
        })
    {
        return Err("Invalid image or histogram size");
    }
    let mut result = Histogram {
        channels: vec![
            ChannelHistogram {
                min: f64::INFINITY,
                max: f64::NEG_INFINITY,
                bins: vec![0; bins]
            };
            channels as usize
        ],
        count: 0,
        missing: 0,
        domain: if frame.color.is_some() {
            "RGB codes before ICC conversion"
        } else if frame.rgba.is_some() {
            "8-bit sRGB presentation codes"
        } else {
            "Scalar source values after stored scaling"
        },
    };
    let values = |index: usize| -> Result<Option<[f64; 3]>, &'static str> {
        if let Some(rgba) = &frame.rgba {
            if !matches!(rgba[index * 4 + 3], 0 | 255) {
                return Err("Invalid color validity mask");
            }
            if rgba[index * 4 + 3] == 0 {
                return Ok(None);
            }
            if let Some(color) = &frame.color {
                let codes = std::array::from_fn(|c| color.rgb[index * 3 + c] as f64);
                if codes.iter().any(|v| *v > f64::from(color.info.maximum())) {
                    return Err("RGB codes exceed their declared precision");
                }
                Ok(Some(codes))
            } else {
                Ok(Some(std::array::from_fn(|c| rgba[index * 4 + c] as f64)))
            }
        } else {
            let value = frame.pixels[index];
            Ok(frame.has_sample(index).then_some([f64::from(value); 3]))
        }
    };
    for index in 0..count as usize {
        if index % 4096 == 0 && cancelled() {
            return Err("Cancelled");
        }
        if let Some(values) = values(index)? {
            result.count += 1;
            for (channel, value) in result.channels.iter_mut().zip(values) {
                channel.min = channel.min.min(value);
                channel.max = channel.max.max(value);
            }
        } else {
            result.missing += 1;
        }
    }
    if result.count == 0 {
        return Err("This image has no valid samples");
    }
    for index in 0..count as usize {
        if index % 4096 == 0 && cancelled() {
            return Err("Cancelled");
        }
        if let Some(values) = values(index)? {
            for (channel, value) in result.channels.iter_mut().zip(values) {
                let span = channel.max - channel.min;
                let bin = if span == 0. {
                    0
                } else {
                    (((value - channel.min) / span * bins as f64) as usize).min(bins - 1)
                };
                channel.bins[bin] += 1;
            }
        }
    }
    if cancelled() {
        return Err("Cancelled");
    }
    Ok(result)
}
