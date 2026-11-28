//! Module manifests (`__manifest__.py`), as written.
//!
//! Odoo's defaults and rules (for example an empty `depends` meaning `base`)
//! are not applied here: they depend on the target version, so the indexer
//! applies them.

use std::fmt;

/// Technical name of an Odoo module: its directory name, for example
/// `sale_stock`. Not the human-readable `name` in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModuleName(String);

impl ModuleName {
    /// Wraps a module's technical name.
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// The name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModuleName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// `auto_install` as written.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AutoInstall {
    /// `False`, or not written: installed only on request.
    #[default]
    No,
    /// `True`: installed once every module in `depends` is installed.
    WithAllDepends,
    /// A list: installed once these modules (a subset of `depends`) are installed.
    When(Vec<ModuleName>),
}

/// The manifest keys Pontix reads, exactly as written.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Manifest {
    /// `name`: the human-readable title.
    pub name: Option<String>,
    /// `version`, for example `19.0.1.0.0` or the short form `1.0`.
    pub version: Option<String>,
    /// `depends`, in declaration order.
    pub depends: Vec<ModuleName>,
    /// `data`: data files relative to the module directory, in load order.
    pub data: Vec<String>,
    /// `demo`: demo data files, loaded after `data` when demo data is on.
    pub demo: Vec<String>,
    /// `installable`; `None` when not written.
    pub installable: Option<bool>,
    /// `auto_install`.
    pub auto_install: AutoInstall,
    /// `external_dependencies["python"]`: Python requirements, as written.
    pub external_python: Vec<String>,
    /// `countries`: country codes, as written. An `auto_install` module with
    /// countries is only installed for a company in one of them.
    pub countries: Vec<String>,
}
