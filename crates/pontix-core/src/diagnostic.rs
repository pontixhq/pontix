//! Diagnostics: what a parser saw but could not turn into symbols.
//!
//! Nothing is dropped silently. Whatever Pontix cannot read is reported with
//! its exact location and source text, so a report never claims more than it read.

use crate::symbol::Location;

/// What kind of construct could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagnosticKind {
    /// The file is not valid Python.
    SyntaxError,
    /// `_name` is set to something other than a string literal.
    UnrecognizedName,
    /// `_inherit` is set to something other than a string or a list of strings.
    UnrecognizedInherit,
    /// `_inherits` is set to something other than a dict of strings.
    UnrecognizedInherits,
    /// A class sets `_name`, `_inherit` or `_inherits`, but none of its bases
    /// is an Odoo model class Pontix recognises.
    UnrecognizedModelClass,
    /// A class attribute looks like a field but is not written `fields.X(...)`.
    UnrecognizedField,
}

/// Something a parser could not read, with the source text exactly as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// What could not be read.
    pub kind: DiagnosticKind,
    /// Where it is.
    pub location: Location,
    /// The source text exactly as written, unchanged.
    pub snippet: String,
}
