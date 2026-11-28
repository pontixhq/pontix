//! Core data types for Pontix.
//!
//! This crate will hold the shared vocabulary of the project: symbols extracted
//! from Odoo code (models, fields, methods, views, XML ids), the changes between
//! two Odoo versions, and the findings reported to users (PTX codes).
#![forbid(unsafe_code)]

pub mod change;
pub mod data;
pub mod diagnostic;
pub mod field_type;
pub mod finding;
pub mod manifest;
pub mod resolve;
pub mod storage;
pub mod symbol;
pub mod version;

pub use change::{Change, ChangeSet, Confidence, Evidence, Rename};
pub use data::{ArchField, DataFile, Record, RefKind, View, XmlRef};
pub use diagnostic::{Diagnostic, DiagnosticKind};
pub use field_type::FieldType;
pub use finding::{Basis, Category, CoreBasis, Finding, PtxCode, Severity, Suggestion};
pub use manifest::{AutoInstall, Manifest, ModuleName};
pub use resolve::ModelKind;
pub use storage::Storage;
pub use symbol::{
    ApiDecorator, Delegation, FieldAttrs, FieldDef, FieldName, FieldPaths, Inherit, Location,
    MethodDef, ModelBase, ModelDef, ModelName, Written,
};
pub use version::OdooVersion;

/// Version of the Pontix core crate.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::VERSION;

    #[test]
    fn version_is_set() {
        assert!(!VERSION.is_empty());
    }
}
