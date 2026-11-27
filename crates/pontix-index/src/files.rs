//! Reading files under an addons path without leaving it.

use std::fs;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use pontix_core::{Diagnostic, DiagnosticKind, Location};

/// Largest file Pontix reads: 10 MiB. Odoo source files are far smaller;
/// anything bigger is reported instead of loaded into memory.
pub const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;

/// One addons path. Every path read through it must stay inside it once
/// symbolic links are resolved.
pub(crate) struct Root {
    canonical: PathBuf,
}

impl Root {
    pub(crate) fn new(path: &Path) -> Result<Self, Diagnostic> {
        fs::canonicalize(path)
            .map(|canonical| Self { canonical })
            .map_err(|_| whole_file(DiagnosticKind::UnreadableFile, path))
    }

    /// The canonical form of `path` when it exists and stays inside the root.
    /// `Ok(None)` when it does not exist.
    pub(crate) fn resolve(&self, path: &Path) -> Result<Option<PathBuf>, Diagnostic> {
        let Ok(canonical) = fs::canonicalize(path) else {
            return Ok(None);
        };
        if canonical.starts_with(&self.canonical) {
            Ok(Some(canonical))
        } else {
            Err(whole_file(DiagnosticKind::OutsideRoot, path))
        }
    }

    /// Reads a text file inside the root. A leading UTF-8 byte order mark is
    /// dropped, as Python does.
    pub(crate) fn read(&self, path: &Path) -> Result<String, Diagnostic> {
        let canonical = self
            .resolve(path)?
            .ok_or_else(|| whole_file(DiagnosticKind::UnreadableFile, path))?;
        let size = fs::metadata(&canonical)
            .map_err(|_| whole_file(DiagnosticKind::UnreadableFile, path))?
            .len();
        if size > MAX_FILE_SIZE {
            return Err(whole_file(DiagnosticKind::FileTooLarge, path));
        }
        let bytes =
            fs::read(&canonical).map_err(|_| whole_file(DiagnosticKind::UnreadableFile, path))?;
        let text = String::from_utf8(bytes)
            .map_err(|_| whole_file(DiagnosticKind::UnreadableFile, path))?;
        Ok(match text.strip_prefix('\u{feff}') {
            Some(rest) => rest.to_string(),
            None => text,
        })
    }
}

/// A diagnostic about a whole file or directory.
pub(crate) fn whole_file(kind: DiagnosticKind, path: &Path) -> Diagnostic {
    Diagnostic {
        kind,
        location: Location::new(path, NonZeroU32::MIN),
        snippet: String::new(),
    }
}
