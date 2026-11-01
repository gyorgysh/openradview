// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded local import explanations without paths, identifiers or parser text.
use crate::load::LoadError;

const MAX_ISSUES: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    UnrecognizedInput,
    UnsupportedEncoding,
    UnsupportedObject,
    MissingCompanion,
    MalformedMetadata,
    UnsupportedOrInvalid,
    MissingReference,
    ConflictingReference,
}

impl SkipReason {
    pub const ALL: [Self; 8] = [
        Self::UnrecognizedInput,
        Self::UnsupportedEncoding,
        Self::UnsupportedObject,
        Self::MissingCompanion,
        Self::MalformedMetadata,
        Self::UnsupportedOrInvalid,
        Self::MissingReference,
        Self::ConflictingReference,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::UnrecognizedInput => "Unrecognized or non-image input",
            Self::UnsupportedEncoding => "Unsupported encoding",
            Self::UnsupportedObject => "Unsupported DICOM object type",
            Self::MissingCompanion => "Missing NIfTI companion",
            Self::MalformedMetadata => "Incomplete or malformed DICOM metadata",
            Self::UnsupportedOrInvalid => "Unsupported or invalid image representation",
            Self::MissingReference => "Missing file-set reference",
            Self::ConflictingReference => "Conflicting file-set image identity",
        }
    }

    fn index(self) -> usize {
        match self {
            Self::UnrecognizedInput => 0,
            Self::UnsupportedEncoding => 1,
            Self::UnsupportedObject => 2,
            Self::MissingCompanion => 3,
            Self::MalformedMetadata => 4,
            Self::UnsupportedOrInvalid => 5,
            Self::MissingReference => 6,
            Self::ConflictingReference => 7,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImportIssue {
    /// One-based position in this import, not a filename or persistent ID.
    pub item: usize,
    pub reason: SkipReason,
}

#[derive(Clone, Debug, Default)]
pub struct ImportReport {
    indexed_files: usize,
    counts: [usize; 8],
    directories: usize,
    issues: Vec<ImportIssue>,
}

impl ImportReport {
    pub fn directories(&self) -> usize {
        self.directories
    }
    pub(crate) fn directory(&mut self) {
        self.directories += 1;
    }
    pub fn indexed_files(&self) -> usize {
        self.indexed_files
    }

    pub fn count(&self, reason: SkipReason) -> usize {
        self.counts[reason.index()]
    }

    pub fn skipped_files(&self) -> usize {
        self.counts.iter().sum()
    }

    pub fn issues(&self) -> &[ImportIssue] {
        &self.issues
    }

    pub fn omitted_issues(&self) -> usize {
        self.skipped_files().saturating_sub(self.issues.len())
    }

    pub(crate) fn indexed(&mut self) {
        self.indexed_files += 1;
    }

    pub(crate) fn skip(&mut self, item: usize, reason: SkipReason) {
        self.counts[reason.index()] += 1;
        if self.issues.len() < MAX_ISSUES {
            self.issues.push(ImportIssue { item, reason });
        }
    }
}

#[derive(Clone, Debug)]
pub struct ImportError {
    pub error: LoadError,
    pub report: ImportReport,
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)?;
        for reason in SkipReason::ALL {
            let count = self.report.count(reason);
            if count > 0 {
                write!(f, " {count} {}.", reason.label().to_lowercase())?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for ImportError {}

impl From<LoadError> for ImportError {
    fn from(error: LoadError) -> Self {
        Self {
            error,
            report: ImportReport::default(),
        }
    }
}

pub(crate) struct InputFailure {
    pub error: LoadError,
    pub reason: SkipReason,
}

impl InputFailure {
    pub fn skipped(reason: SkipReason) -> Self {
        Self {
            error: LoadError::Unsupported,
            reason,
        }
    }
}

impl From<LoadError> for InputFailure {
    fn from(error: LoadError) -> Self {
        Self {
            error,
            reason: SkipReason::UnsupportedOrInvalid,
        }
    }
}
