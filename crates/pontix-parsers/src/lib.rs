//! Parsers for Odoo source files.
//!
//! Python files (models, fields, methods) and XML files (views, templates,
//! records) are parsed here and turned into `pontix-core` symbols. The parser
//! crates used underneath never appear in this crate's public API, so they can
//! be replaced without touching callers.
#![forbid(unsafe_code)]

pub mod python;

pub use python::{PythonFile, parse_python};
