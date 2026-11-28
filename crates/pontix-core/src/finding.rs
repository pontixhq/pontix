//! Findings: what Pontix reports to the user, each with a stable PTX code, a
//! severity, and evidence. A finding cannot exist without its [`Basis`],
//! which can only be built from where core declares what changed, from the
//! Odoo rule that changed, or from core having it in neither version.

use std::fmt;

use crate::change::Confidence;
use crate::manifest::ModuleName;
use crate::symbol::Location;

/// A PTX code. Released codes are never renumbered or reused: people keep
/// them in CI configuration and notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PtxCode {
    /// A module in `depends` is gone or renamed.
    M001,
    /// The manifest version is of another series: Odoo skips the module
    /// without an error.
    M002,
    /// A `migrations/` folder holds scripts for versions below the target
    /// series: they never run again.
    M003,
    /// A class with an `_inherit` list and no `_name` gets another model
    /// name.
    M004,
    /// The manifest is not one literal expression: Odoo ignores the module.
    M005,
    /// The module only has `__openerp__.py`, which the target no longer
    /// reads.
    M006,
    /// An import inside the module points to nothing: the module fails to
    /// load.
    M007,
    /// A model the module uses is gone.
    M101,
    /// A model the module uses is probably merged or renamed.
    M102,
    /// A model the module defines now also exists in core.
    M103,
    /// A field the module refers to is gone.
    F201,
    /// A field the module refers to is probably renamed.
    F202,
    /// A core field the module declares again has another type.
    F203,
    /// A core field the module declares again is gone from core.
    F205,
    /// A core method the module overrides with `super()` is gone.
    P401,
    /// A core method the module overrides takes other parameters.
    P402,
    /// A core method the module overrides does something else.
    P403,
    /// A core method the module overrides without `super()` is gone.
    P404,
    /// A framework name or model method the module uses is gone.
    A101,
    /// A framework name or model method the module uses is deprecated.
    A102,
    /// A record the module refers to by external id is gone.
    R601,
    /// A test relies on a record only demo data creates, and the target
    /// makes databases without demo data by default.
    T802,
    /// A stored column or table goes away, and its data with it.
    D901,
    /// A selection field no longer accepts keys rows may hold.
    D902,
    /// A stored field changes type: its data must be converted.
    D903,
}

impl PtxCode {
    /// The code as written, such as `PTX-F201`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::M001 => "PTX-M001",
            Self::M002 => "PTX-M002",
            Self::M003 => "PTX-M003",
            Self::M004 => "PTX-M004",
            Self::M005 => "PTX-M005",
            Self::M006 => "PTX-M006",
            Self::M007 => "PTX-M007",
            Self::M101 => "PTX-M101",
            Self::M102 => "PTX-M102",
            Self::M103 => "PTX-M103",
            Self::F201 => "PTX-F201",
            Self::F202 => "PTX-F202",
            Self::F203 => "PTX-F203",
            Self::F205 => "PTX-F205",
            Self::P401 => "PTX-P401",
            Self::P402 => "PTX-P402",
            Self::P403 => "PTX-P403",
            Self::P404 => "PTX-P404",
            Self::A101 => "PTX-A101",
            Self::A102 => "PTX-A102",
            Self::R601 => "PTX-R601",
            Self::T802 => "PTX-T802",
            Self::D901 => "PTX-D901",
            Self::D902 => "PTX-D902",
            Self::D903 => "PTX-D903",
        }
    }

    /// The severity the code has when the upgrade causes it.
    pub fn severity(self) -> Severity {
        match self {
            Self::M003
            | Self::F205
            | Self::P402
            | Self::P403
            | Self::P404
            | Self::A102
            | Self::T802
            | Self::D902
            | Self::D903 => Severity::Warning,
            _ => Severity::Error,
        }
    }

    /// What kind of harm it is about.
    pub fn category(self) -> Category {
        match self {
            Self::D901 => Category::DataLoss,
            Self::D902 | Self::D903 => Category::DataRisk,
            Self::F205 | Self::P402 | Self::P403 | Self::P404 => Category::Behavior,
            Self::M003 => Category::Housekeeping,
            _ => Category::CodeBreak,
        }
    }
}

impl fmt::Display for PtxCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How much a finding matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Breaks the module or loses data.
    Error,
    /// Changes what the module does, or risks data.
    Warning,
    /// Worth knowing; not caused by the upgrade, or harmless.
    Info,
}

impl Severity {
    /// `error`, `warning` or `info`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
        }
    }
}

/// What kind of harm a finding is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    /// Code or data files that fail to load or run.
    CodeBreak,
    /// Data the upgrade removes.
    DataLoss,
    /// Data that needs converting or checking.
    DataRisk,
    /// Code that runs but may do something else.
    Behavior,
    /// Code or files left behind that no longer do anything.
    Housekeeping,
}

impl Category {
    /// A stable name, such as `code_break`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CodeBreak => "code_break",
            Self::DataLoss => "data_loss",
            Self::DataRisk => "data_risk",
            Self::Behavior => "behavior",
            Self::Housekeeping => "housekeeping",
        }
    }
}

/// Why Pontix reports a finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Basis {
    /// Where core declares what changed.
    Core(CoreBasis),
    /// An Odoo rule that differs between the versions, cited from Odoo's
    /// source.
    Rule(&'static str),
    /// Core has what is referred to in neither version: the reference was
    /// broken before the upgrade.
    NotInCore,
}

/// Where core declares what changed: in the source version, the target
/// version, or both. Never neither.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreBasis {
    before: Option<Location>,
    after: Option<Location>,
}

impl CoreBasis {
    /// `None` when neither location is known: that is no evidence.
    pub fn new(before: Option<Location>, after: Option<Location>) -> Option<Self> {
        (before.is_some() || after.is_some()).then_some(Self { before, after })
    }

    /// In the source version.
    pub fn before(&self) -> Option<&Location> {
        self.before.as_ref()
    }

    /// In the target version; `None` when it is gone.
    pub fn after(&self) -> Option<&Location> {
        self.after.as_ref()
    }
}

/// What the module should probably refer to instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    /// The likely new name, such as `sale.order.line.tax_ids`.
    pub rename_to: String,
    /// How strongly the evidence points to it.
    pub confidence: Confidence,
}

/// One thing the upgrade breaks or changes for a module, with every place
/// it shows up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The PTX code.
    pub code: PtxCode,
    /// How much it matters.
    pub severity: Severity,
    /// What kind of harm.
    pub category: Category,
    /// The module affected: a module being upgraded, or the core module
    /// whose data changes.
    pub module: ModuleName,
    /// What it is about, such as `sale.order.line.tax_id`.
    pub subject: String,
    /// One sentence for people.
    pub message: String,
    /// Every place it shows up, in file and line order.
    pub locations: Vec<Location>,
    /// Why it is reported.
    pub basis: Basis,
    /// What to use instead, when Pontix suspects a rename.
    pub suggestion: Option<Suggestion>,
}
