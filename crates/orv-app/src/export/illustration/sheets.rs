// SPDX-License-Identifier: GPL-3.0-or-later
//! Captured sheet scope and explicit source ordinals, without eager tile decoding.
use super::*;
use crate::annotations::SourceOutlines;
use orv_render::snapshot::mosaic::Layout;

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
pub(super) struct SourceRange {
    pub first: u32,
    pub through: u32,
    pub step: u32,
}
impl SourceRange {
    /// Controls use one-based numbers; scenes and source reads use zero-based
    /// ordinals. A through value bounds the range and need not itself be sampled.
    pub fn ordinals(self, total: usize) -> Result<Vec<u32>, &'static str> {
        if self.first == 0
            || self.step == 0
            || self.through < self.first
            || self.through as usize > total
        {
            return Err("Choose an image range within the captured source");
        }
        let count = (u64::from(self.through) - u64::from(self.first)) / u64::from(self.step) + 1;
        if count > 16 {
            return Err(
                "A contact sheet can include at most 16 images; shorten the range or increase the step",
            );
        }
        let mut ordinals = Vec::new();
        ordinals
            .try_reserve_exact(count as usize)
            .map_err(|_| "Not enough memory for image ordinals")?;
        for index in 0..count {
            ordinals.push((u64::from(self.first) - 1 + index * u64::from(self.step)) as u32);
        }
        Ok(ordinals)
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
pub(super) struct Settings {
    pub columns: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<SourceRange>,
}
pub(crate) enum Sheet {
    Source { marks: SourceOutlines, total: usize },
    Mpr { panes: Vec<FrozenInput> },
}
impl Sheet {
    pub(super) fn defaults(&self) -> Settings {
        match self {
            Self::Source { total, .. } => Settings {
                columns: (*total).min(4) as u32,
                range: Some(SourceRange {
                    first: 1,
                    through: (*total).min(16) as u32,
                    step: 1,
                }),
            },
            Self::Mpr { .. } => Settings {
                columns: 2,
                range: None,
            },
        }
    }
    pub(super) fn count_marks(&self) -> usize {
        match self {
            Self::Source { marks, .. } => marks.count(),
            Self::Mpr { panes } => panes.iter().map(|p| p.outlines.marks().len()).sum(),
        }
    }
    pub(super) fn layout(&self, settings: Settings) -> Result<(Layout, Vec<u32>), &'static str> {
        match self {
            Self::Source { total, .. } => {
                let ordinals = settings
                    .range
                    .ok_or("Choose source image numbers for this sheet")?
                    .ordinals(*total)?;
                if settings.columns as usize > ordinals.len() {
                    return Err("Choose no more columns than selected images");
                }
                let sizes = [[512, 512]; 16];
                Ok((
                    Layout::new(&sizes[..ordinals.len()], settings.columns)?,
                    ordinals,
                ))
            }
            Self::Mpr { panes } => {
                if panes.len() != 3 || settings.range.is_some() {
                    return Err("MPR sheets need exactly three captured planes");
                }
                if settings.columns > 3 {
                    return Err("Choose one to three columns for the MPR grid");
                }
                let sizes = [
                    panes[0].base.scene.output_size,
                    panes[1].base.scene.output_size,
                    panes[2].base.scene.output_size,
                ];
                Ok((Layout::new(&sizes, settings.columns)?, Vec::new()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_source_ranges_keep_the_exact_requested_order_without_silent_clamping() {
        assert_eq!(
            SourceRange {
                first: 1,
                through: 7,
                step: 3
            }
            .ordinals(7)
            .unwrap(),
            [0, 3, 6]
        );
        assert_eq!(
            SourceRange {
                first: 2,
                through: 8,
                step: 4
            }
            .ordinals(8)
            .unwrap(),
            [1, 5]
        );
        assert_eq!(
            SourceRange {
                first: 1,
                through: 16,
                step: 1
            }
            .ordinals(16)
            .unwrap(),
            (0..16).collect::<Vec<_>>()
        );
        assert_eq!(
            SourceRange {
                first: u32::MAX,
                through: u32::MAX,
                step: u32::MAX
            }
            .ordinals(u32::MAX as usize)
            .unwrap(),
            [u32::MAX - 1]
        );
        for range in [
            SourceRange {
                first: 0,
                through: 1,
                step: 1,
            },
            SourceRange {
                first: 1,
                through: 1,
                step: 0,
            },
            SourceRange {
                first: 3,
                through: 2,
                step: 1,
            },
            SourceRange {
                first: 1,
                through: 17,
                step: 1,
            },
            SourceRange {
                first: 1,
                through: 100,
                step: 7,
            },
        ] {
            assert!(range.ordinals(16).is_err());
        }
        assert!(
            SourceRange {
                first: 1,
                through: 17,
                step: 1
            }
            .ordinals(17)
            .unwrap_err()
            .contains("16 images")
        );
        assert!(
            SourceRange {
                first: 1,
                through: 1,
                step: 1
            }
            .ordinals(0)
            .is_err()
        );
    }
}
