//! Parsers for Odoo source files.
//!
//! Python files (models, fields, methods) and XML files (views, templates,
//! records) are parsed here and turned into `pontix-core` symbols. The parser
//! crates used underneath never appear in this crate's public API, so they can
//! be replaced without touching callers.
#![forbid(unsafe_code)]

pub mod csv;
pub mod manifest;
pub mod python;
pub mod qweb;
pub mod release;
mod source;
pub mod xml;
pub mod xpath;

pub use csv::{CsvFile, parse_csv};
pub use manifest::{ManifestFile, parse_manifest};
pub use python::framework::{
    Binding, BindingKind, FrameworkClass, FrameworkFile, Member, parse_framework,
};
pub use python::{Candidate, Import, PythonFile, parse_python, parse_python_with};
pub use qweb::{Chain, QwebExpr, format_expressions, read_expression};
pub use release::release_version;
pub use xml::{XmlFile, parse_xml};
pub use xpath::{XPath, XPathError};
