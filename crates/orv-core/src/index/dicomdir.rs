// SPDX-License-Identifier: GPL-3.0-or-later
//! Read-only Basic Directory subset. Offsets refer to the original file bytes.
use crate::{
    load::LoadError,
    resources::{MemoryBudget, Reservation},
};
use std::{
    collections::{HashMap, HashSet},
    io::Read,
};

const CLASS: &str = "1.2.840.10008.1.3.10";
const EXPLICIT_LE: &str = "1.2.840.10008.1.2.1";
const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_RECORDS: usize = 50_000;
const UNDEFINED: u32 = u32::MAX;
type Tag = (u16, u16);

pub(super) struct Directory {
    pub references: Vec<Reference>,
    pub bytes: u64,
    _reservation: Reservation,
}
pub(super) struct Reference {
    pub components: Vec<String>,
    pub image: bool,
    class: String,
    instance: String,
    syntax: String,
    study: Option<String>,
    series: Option<String>,
}
impl Reference {
    pub fn matches(&self, header: &super::Header) -> bool {
        let Ok(object) = header.dicom() else {
            return false;
        };
        let meta = object.meta();
        let syntax = if header.deflated_at.is_some() {
            "1.2.840.10008.1.2.1.99"
        } else {
            meta.transfer_syntax()
        };
        let text = |name| crate::load::text(object, name);
        self.class == meta.media_storage_sop_class_uid()
            && self.instance == meta.media_storage_sop_instance_uid()
            && self.syntax == syntax
            && text("SOPClassUID").as_deref() == Some(&self.class)
            && text("SOPInstanceUID").as_deref() == Some(&self.instance)
            && text("StudyInstanceUID") == self.study
            && text("SeriesInstanceUID") == self.series
    }
}

#[derive(Default)]
struct Record {
    next: Option<u32>,
    active: Option<u16>,
    lower: Option<u32>,
    kind: Option<String>,
    file: Option<String>,
    class: Option<String>,
    instance: Option<String>,
    syntax: Option<String>,
    study: Option<String>,
    series: Option<String>,
}

pub(super) fn is_name(path: &std::path::Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("DICOMDIR"))
}

pub(super) fn read(
    reader: &mut impl Read,
    length: u64,
    cancelled: &impl Fn() -> bool,
) -> Result<Directory, LoadError> {
    if length > MAX_BYTES as u64 {
        return Err(LoadError::Limit);
    }
    // Includes the byte buffer, record table, traversal and owned references.
    let reservation = MemoryBudget::volumes()
        .reserve(length * 8 + 8 * 1024 * 1024)
        .ok_or(LoadError::Limit)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length as usize)
        .map_err(|_| LoadError::Limit)?;
    let mut scratch = [0u8; 64 * 1024];
    loop {
        if cancelled() {
            return Err(LoadError::Cancelled);
        }
        let count = reader
            .read(&mut scratch)
            .map_err(|_| LoadError::Directory)?;
        if count == 0 {
            break;
        }
        if bytes.len() + count > MAX_BYTES {
            return Err(LoadError::Limit);
        }
        if bytes.len() + count > length as usize {
            return Err(LoadError::Directory);
        }
        bytes.extend_from_slice(&scratch[..count]);
    }
    if bytes.len() as u64 != length || bytes.get(128..132) != Some(b"DICM") {
        return Err(LoadError::Directory);
    }
    let mut input = Input {
        data: &bytes,
        pos: 132,
        tokens: 0,
        cancelled,
    };
    let mut class = None;
    let mut syntax = None;
    let mut meta_end = None;
    while input.pos < bytes.len() && input.tag()?.0 == 2 {
        let element = input.element()?;
        match element.tag {
            (2, 0) => {
                if meta_end.is_some() || element.start != 132 {
                    return Err(LoadError::Directory);
                }
                meta_end = Some(
                    input
                        .pos
                        .checked_add(element.ul()? as usize)
                        .ok_or(LoadError::Limit)?,
                );
            }
            (2, 2) => set(&mut class, element.uid()?)?,
            (2, 0x10) => set(&mut syntax, element.uid()?)?,
            _ => input.skip(element, 0)?,
        }
    }
    if meta_end != Some(input.pos)
        || class.as_deref() != Some(CLASS)
        || syntax.as_deref() != Some(EXPLICIT_LE)
    {
        return Err(LoadError::Directory);
    }
    let (mut first, mut last, mut consistency, mut records) = (None, None, None, None);
    let mut previous = None;
    while input.pos < bytes.len() {
        let element = input.element()?;
        ascending(&mut previous, element.tag)?;
        match element.tag {
            (4, 0x1200) => set(&mut first, element.ul()?)?,
            (4, 0x1202) => set(&mut last, element.ul()?)?,
            (4, 0x1212) => set(&mut consistency, element.us()?)?,
            (4, 0x1220) => {
                if element.vr != *b"SQ" {
                    return Err(LoadError::Directory);
                }
                set(&mut records, input.records(element)?)?;
            }
            _ => input.skip(element, 0)?,
        }
    }
    let references = match (first, last, consistency, records) {
        (None, None, None, None) => Vec::new(), // Directory Information is optional.
        (Some(first), Some(last), Some(0), Some(records)) => {
            traverse(records, first, last, cancelled)?
        }
        _ => return Err(LoadError::Directory),
    };
    Ok(Directory {
        references,
        bytes: length,
        _reservation: reservation,
    })
}

fn set<T>(target: &mut Option<T>, value: T) -> Result<(), LoadError> {
    if target.replace(value).is_some() {
        return Err(LoadError::Directory);
    }
    Ok(())
}
fn ascending(previous: &mut Option<Tag>, tag: Tag) -> Result<(), LoadError> {
    if previous.is_some_and(|value| value >= tag) {
        return Err(LoadError::Directory);
    }
    *previous = Some(tag);
    Ok(())
}

struct Element<'a> {
    tag: Tag,
    vr: [u8; 2],
    start: usize,
    length: u32,
    value: &'a [u8],
}
impl Element<'_> {
    fn ul(&self) -> Result<u32, LoadError> {
        if self.vr != *b"UL" || self.value.len() != 4 {
            return Err(LoadError::Directory);
        }
        Ok(u32::from_le_bytes(
            self.value.try_into().map_err(|_| LoadError::Directory)?,
        ))
    }
    fn us(&self) -> Result<u16, LoadError> {
        if self.vr != *b"US" || self.value.len() != 2 {
            return Err(LoadError::Directory);
        }
        Ok(u16::from_le_bytes(
            self.value.try_into().map_err(|_| LoadError::Directory)?,
        ))
    }
    fn text(&self, vr: &[u8; 2], limit: usize) -> Result<String, LoadError> {
        if self.vr != *vr || self.value.len() > limit || !self.value.is_ascii() {
            return Err(LoadError::Directory);
        }
        let value = std::str::from_utf8(self.value).map_err(|_| LoadError::Directory)?;
        Ok(value.trim_end_matches([' ', '\0']).to_owned())
    }
    fn uid(&self) -> Result<String, LoadError> {
        let value = self.text(b"UI", 64)?;
        if !crate::load::valid_identifier(&value) {
            return Err(LoadError::Directory);
        }
        Ok(value)
    }
}

struct Input<'a, F> {
    data: &'a [u8],
    pos: usize,
    tokens: usize,
    cancelled: &'a F,
}
impl<'a, F: Fn() -> bool> Input<'a, F> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], LoadError> {
        let end = self.pos.checked_add(count).ok_or(LoadError::Limit)?;
        let value = self.data.get(self.pos..end).ok_or(LoadError::Directory)?;
        self.pos = end;
        Ok(value)
    }
    fn tag(&self) -> Result<Tag, LoadError> {
        let value = self
            .data
            .get(self.pos..self.pos + 4)
            .ok_or(LoadError::Directory)?;
        Ok((
            u16::from_le_bytes([value[0], value[1]]),
            u16::from_le_bytes([value[2], value[3]]),
        ))
    }
    fn element(&mut self) -> Result<Element<'a>, LoadError> {
        if (self.cancelled)() {
            return Err(LoadError::Cancelled);
        }
        self.tokens += 1;
        if self.tokens > 1_000_000 {
            return Err(LoadError::Limit);
        }
        let start = self.pos;
        let tag = self.tag()?;
        if tag.0 == 0xfffe || tag.0 == 0x7fe0 {
            return Err(LoadError::Directory);
        }
        let head = self.take(8)?;
        let vr = [head[4], head[5]];
        let length = match &vr {
            b"OB" | b"OD" | b"OF" | b"OL" | b"OV" | b"OW" | b"SQ" | b"SV" | b"UC" | b"UN"
            | b"UR" | b"UT" | b"UV" => {
                if head[6..8] != [0, 0] {
                    return Err(LoadError::Directory);
                }
                u32::from_le_bytes(self.take(4)?.try_into().map_err(|_| LoadError::Directory)?)
            }
            b"AE" | b"AS" | b"AT" | b"CS" | b"DA" | b"DS" | b"DT" | b"FD" | b"FL" | b"IS"
            | b"LO" | b"LT" | b"PN" | b"SH" | b"SL" | b"SS" | b"ST" | b"TM" | b"UI" | b"UL"
            | b"US" => u16::from_le_bytes([head[6], head[7]]) as u32,
            _ => return Err(LoadError::Directory),
        };
        if length != UNDEFINED && !length.is_multiple_of(2) {
            return Err(LoadError::Directory);
        }
        let value = if vr == *b"SQ" {
            &[]
        } else {
            if length == UNDEFINED {
                return Err(LoadError::Directory);
            }
            self.take(length as usize)?
        };
        Ok(Element {
            tag,
            vr,
            start,
            length,
            value,
        })
    }
    fn item(&mut self, expected: u16) -> Result<u32, LoadError> {
        if (self.cancelled)() {
            return Err(LoadError::Cancelled);
        }
        self.tokens += 1;
        if self.tokens > 1_000_000 {
            return Err(LoadError::Limit);
        }
        if self.tag()? != (0xfffe, expected) {
            return Err(LoadError::Directory);
        }
        let bytes = self.take(8)?;
        Ok(u32::from_le_bytes(
            bytes[4..8].try_into().map_err(|_| LoadError::Directory)?,
        ))
    }
    fn end(&mut self, end: Option<usize>, delimiter: u16) -> Result<bool, LoadError> {
        if let Some(end) = end {
            if self.pos > end {
                return Err(LoadError::Directory);
            }
            return Ok(self.pos == end);
        }
        if self.tag()? == (0xfffe, delimiter) {
            if self.item(delimiter)? != 0 {
                return Err(LoadError::Directory);
            }
            return Ok(true);
        }
        Ok(false)
    }
    fn bound(&self, length: u32) -> Result<Option<usize>, LoadError> {
        if length == UNDEFINED {
            return Ok(None);
        }
        if !length.is_multiple_of(2) {
            return Err(LoadError::Directory);
        }
        let end = self
            .pos
            .checked_add(length as usize)
            .ok_or(LoadError::Limit)?;
        if end > self.data.len() {
            return Err(LoadError::Directory);
        }
        Ok(Some(end))
    }
    fn skip(&mut self, element: Element<'a>, depth: usize) -> Result<(), LoadError> {
        if element.vr != *b"SQ" {
            return Ok(());
        }
        if depth >= 16 {
            return Err(LoadError::Limit);
        }
        let end = self.bound(element.length)?;
        while !self.end(end, 0xe0dd)? {
            let length = self.item(0xe000)?;
            let item_end = self.bound(length)?;
            while !self.end(item_end, 0xe00d)? {
                let element = self.element()?;
                self.skip(element, depth + 1)?;
            }
        }
        Ok(())
    }
    fn records(&mut self, element: Element<'a>) -> Result<HashMap<u32, Record>, LoadError> {
        let end = self.bound(element.length)?;
        let mut records = HashMap::new();
        while !self.end(end, 0xe0dd)? {
            if records.len() >= MAX_RECORDS {
                return Err(LoadError::Limit);
            }
            let offset = u32::try_from(self.pos).map_err(|_| LoadError::Limit)?;
            let length = self.item(0xe000)?;
            let item_end = self.bound(length)?;
            let mut record = Record::default();
            let mut previous = None;
            while !self.end(item_end, 0xe00d)? {
                let element = self.element()?;
                ascending(&mut previous, element.tag)?;
                match element.tag {
                    (4, 0x1400) => set(&mut record.next, element.ul()?)?,
                    (4, 0x1410) => set(&mut record.active, element.us()?)?,
                    (4, 0x1420) => set(&mut record.lower, element.ul()?)?,
                    (4, 0x1430) => set(&mut record.kind, element.text(b"CS", 16)?)?,
                    (4, 0x1500) => set(&mut record.file, element.text(b"CS", 72)?)?,
                    (4, 0x1504) => return Err(LoadError::Directory), // Retired indirect references.
                    (4, 0x1510) => set(&mut record.class, element.uid()?)?,
                    (4, 0x1511) => set(&mut record.instance, element.uid()?)?,
                    (4, 0x1512) => set(&mut record.syntax, element.uid()?)?,
                    (0x20, 0xd) => set(&mut record.study, element.uid()?)?,
                    (0x20, 0xe) => set(&mut record.series, element.uid()?)?,
                    _ => self.skip(element, 0)?,
                }
            }
            if record.next.is_none()
                || record.lower.is_none()
                || record.active.is_none()
                || record.kind.is_none()
            {
                return Err(LoadError::Directory);
            }
            records.insert(offset, record);
        }
        Ok(records)
    }
}

fn traverse(
    records: HashMap<u32, Record>,
    first: u32,
    last: u32,
    cancelled: &impl Fn() -> bool,
) -> Result<Vec<Reference>, LoadError> {
    if (first == 0) != (last == 0) {
        return Err(LoadError::Directory);
    }
    let mut pending = vec![(first, None, None, None, 0usize)];
    let mut visited = HashSet::new();
    let mut files = HashSet::new();
    let mut instances = HashSet::new();
    let mut references = Vec::new();
    let mut final_root = 0;
    while let Some((offset, parent, study, series, depth)) = pending.pop() {
        if cancelled() {
            return Err(LoadError::Cancelled);
        }
        if offset == 0 {
            continue;
        }
        if depth >= 16 {
            return Err(LoadError::Limit);
        }
        if !visited.insert(offset) {
            return Err(LoadError::Directory);
        }
        let record = records.get(&offset).ok_or(LoadError::Directory)?;
        let next = record.next.ok_or(LoadError::Directory)?;
        let lower = record.lower.ok_or(LoadError::Directory)?;
        let kind = record.kind.as_deref().ok_or(LoadError::Directory)?;
        if parent.is_none() {
            final_root = offset
        }
        pending.push((next, parent, study, series, depth));
        if record.active == Some(0) {
            if lower != 0 {
                return Err(LoadError::Directory);
            }
            continue;
        }
        let parent_kind = parent
            .and_then(|offset| records.get(&offset))
            .and_then(|r| r.kind.as_deref());
        let allowed = match parent_kind {
            None => matches!(
                kind,
                "PATIENT"
                    | "HANGING PROTOCOL"
                    | "PALETTE"
                    | "IMPLANT"
                    | "IMPLANT ASSY"
                    | "IMPLANT GROUP"
                    | "INVENTORY"
                    | "PRIVATE"
            ),
            Some("PATIENT") => matches!(kind, "STUDY" | "PRIVATE"),
            Some("STUDY") => matches!(kind, "SERIES" | "PRIVATE"),
            Some("SERIES") => matches!(
                kind,
                "IMAGE"
                    | "RT DOSE"
                    | "RT STRUCTURE SET"
                    | "RT PLAN"
                    | "RT TREAT RECORD"
                    | "PRESENTATION"
                    | "WAVEFORM"
                    | "SR DOCUMENT"
                    | "KEY OBJECT DOC"
                    | "SPECTROSCOPY"
                    | "RAW DATA"
                    | "REGISTRATION"
                    | "FIDUCIAL"
                    | "ENCAP DOC"
                    | "VALUE MAP"
                    | "STEREOMETRIC"
                    | "PLAN"
                    | "MEASUREMENT"
                    | "SURFACE"
                    | "TRACT"
                    | "ASSESSMENT"
                    | "RADIOTHERAPY"
                    | "ANNOTATION"
                    | "WF PRESENTATION"
                    | "PRIVATE"
            ),
            Some(_) => kind == "PRIVATE",
        };
        if !allowed {
            return Err(LoadError::Directory);
        }
        let study = if kind == "STUDY" { Some(offset) } else { study };
        let series = if kind == "SERIES" {
            Some(offset)
        } else {
            series
        };
        pending.push((lower, Some(offset), study, series, depth + 1));
        if let Some(file) = &record.file {
            let components = file_components(file)?;
            if !files.insert(components.join("/").to_ascii_lowercase()) {
                return Err(LoadError::Directory);
            }
            let instance = record.instance.clone().ok_or(LoadError::Directory)?;
            if !instances.insert(instance.clone()) {
                return Err(LoadError::Directory);
            }
            let study = study
                .and_then(|offset| records.get(&offset))
                .and_then(|r| r.study.clone());
            let series = series
                .and_then(|offset| records.get(&offset))
                .and_then(|r| r.series.clone());
            if kind == "IMAGE" && (study.is_none() || series.is_none()) {
                return Err(LoadError::Directory);
            }
            references.push(Reference {
                components,
                image: kind == "IMAGE",
                instance,
                class: record.class.clone().ok_or(LoadError::Directory)?,
                syntax: record.syntax.clone().ok_or(LoadError::Directory)?,
                study,
                series,
            });
        } else if kind == "IMAGE"
            || record.class.is_some()
            || record.instance.is_some()
            || record.syntax.is_some()
        {
            return Err(LoadError::Directory);
        }
    }
    if final_root != last
        || records
            .iter()
            .any(|(offset, record)| record.active != Some(0) && !visited.contains(offset))
    {
        return Err(LoadError::Directory);
    }
    Ok(references)
}

fn file_components(file: &str) -> Result<Vec<String>, LoadError> {
    let components: Vec<_> = file.split('\\').collect();
    if components.is_empty()
        || components.len() > 8
        || components.iter().any(|part| {
            part.is_empty()
                || part.len() > 8
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
    {
        return Err(LoadError::Directory);
    }
    Ok(components.into_iter().map(str::to_owned).collect())
}
