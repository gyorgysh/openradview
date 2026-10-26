// SPDX-License-Identifier: GPL-3.0-or-later
//! Cancellable gzip/pair preparation shared by local files and ZIP members.
use super::{
    Progress,
    header::Header,
    import::MAX_EXPANDED,
    source::{Encoding, Source},
};
use crate::load::LoadError;
pub(super) struct NiftiInput {
    pub source: Source,
    pub gzip: bool,
    pub paired: bool,
    pub companion: Option<(Source, bool)>,
}

pub(super) fn read(
    input: NiftiInput,
    id: u64,
    cancelled: &impl Fn() -> bool,
    expanded: &mut u64,
    report: &mut Progress,
    progress: &mut impl FnMut(Progress),
) -> Result<(Source, Vec<Header>), LoadError> {
    let NiftiInput {
        source,
        gzip,
        paired,
        companion,
    } = input;
    let source = if gzip {
        let source =
            source.inflate_gzip(MAX_EXPANDED.saturating_sub(*expanded), cancelled, |bytes| {
                report.checked_bytes = report.checked_bytes.saturating_add(bytes);
                progress(*report);
            })?;
        *expanded = expanded
            .checked_add(source.length)
            .ok_or(LoadError::Limit)?;
        source
    } else {
        source
    };
    if paired {
        let (data, gzip) = companion.ok_or(LoadError::Unsupported)?;
        let data = if gzip {
            let data =
                data.inflate_gzip(MAX_EXPANDED.saturating_sub(*expanded), cancelled, |bytes| {
                    report.checked_bytes = report.checked_bytes.saturating_add(bytes);
                    progress(*report);
                })?;
            *expanded = expanded.checked_add(data.length).ok_or(LoadError::Limit)?;
            data
        } else {
            data
        };
        let headers = super::nifti::read_headers(&source, data.length, true, id, cancelled)?;
        let guarded = Source {
            id: data.id,
            path: data.path.clone(),
            stamp: data.stamp.clone(),
            offset: 0,
            length: data.length,
            compressed: data.compressed,
            encoding: Encoding::Paired {
                data: Box::new(data),
                header: Box::new(source),
            },
        };
        Ok((guarded, headers))
    } else {
        let headers = super::nifti::read_headers(&source, source.length, false, id, cancelled)?;
        Ok((source, headers))
    }
}
