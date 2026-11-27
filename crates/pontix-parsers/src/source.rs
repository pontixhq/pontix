//! A source file being read: offsets become locations, ranges become
//! diagnostics with the text exactly as written.

use std::num::NonZeroU32;
use std::path::Path;

use pontix_core::{Diagnostic, DiagnosticKind, Location};
use ruff_source_file::LineIndex;
use ruff_text_size::{TextRange, TextSize};

pub(crate) struct SourceFile<'a> {
    file: &'a Path,
    text: &'a str,
    lines: LineIndex,
}

impl<'a> SourceFile<'a> {
    pub(crate) fn new(file: &'a Path, text: &'a str) -> Self {
        Self {
            file,
            text,
            lines: LineIndex::from_source_text(text),
        }
    }

    pub(crate) fn location(&self, offset: TextSize) -> Location {
        // Saturates instead of failing: a source file cannot realistically
        // have more than u32::MAX lines.
        let line = u32::try_from(self.lines.line_index(offset).get())
            .ok()
            .and_then(NonZeroU32::new)
            .unwrap_or(NonZeroU32::MAX);
        Location::new(self.file, line)
    }

    /// A diagnostic quoting the text of `range`, exactly as written.
    pub(crate) fn diagnostic(&self, kind: DiagnosticKind, range: TextRange) -> Diagnostic {
        Diagnostic {
            kind,
            location: self.location(range.start()),
            snippet: self.text[range].to_string(),
        }
    }

    /// A diagnostic quoting the whole line containing `offset`, without its
    /// line ending.
    pub(crate) fn line_diagnostic(&self, kind: DiagnosticKind, offset: TextSize) -> Diagnostic {
        let line = self
            .lines
            .line_range(self.lines.line_index(offset), self.text);
        Diagnostic {
            kind,
            location: self.location(offset),
            snippet: self.text[line].trim_end_matches(['\r', '\n']).to_string(),
        }
    }
}
