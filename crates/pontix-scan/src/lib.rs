//! Finds what custom Odoo modules rely on in core, and what of it the upgrade
//! breaks or changes.
//!
//! The modules being upgraded are installed on both versions, still as
//! written for the source version. What they declare is checked against
//! each version as a whole: the models they inherit and point fields at,
//! the field paths of `related=` and `@api` decorators, the core fields they
//! redeclare, the core methods they override, and their dependencies. A
//! reference that works in the source version and fails in the target one
//! is broken by the upgrade; one that already fails in the source version is
//! reported as broken before it, so the upgrade is never blamed for it.
//!
//! Every issue carries where the custom module refers to core and where core
//! declared it in each version. Pontix reads code only: an issue is a risk
//! with its evidence, and no issue never means safe.
#![forbid(unsafe_code)]

mod check;
mod path;

use pontix_core::{
    ChangeSet, Confidence, FieldName, FieldType, Location, ModelName, ModuleName, RefKind,
};
use pontix_index::{Addons, InstallOptions, install, install_with};

/// What the scan found, and the core changes it was checked against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scan {
    /// Issues in the modules being upgraded, by module, then file and line.
    pub issues: Vec<Issue>,
    /// What changed in the core modules the upgraded modules install.
    pub changes: ChangeSet,
}

/// One place where a module being upgraded relies on something the upgrade
/// breaks or changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// The module being upgraded.
    pub module: ModuleName,
    /// Where it refers to what changed.
    pub at: Location,
    /// What is wrong.
    pub problem: Problem,
    /// Whether the upgrade causes it.
    pub since: Since,
    /// Where core declared what is referred to, in each version.
    pub evidence: Evidence,
}

/// Whether the upgrade causes an issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Since {
    /// It works in the source version and fails in the target one.
    Upgrade,
    /// It already fails in the source version.
    Before,
}

/// Where core declared what an issue is about. `after` is `None` when the
/// target version no longer has it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Evidence {
    /// In the source version.
    pub before: Option<Location>,
    /// In the target version.
    pub after: Option<Location>,
}

/// A rename Pontix suspects, with how sure it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Renamed<T> {
    /// What it was probably renamed to.
    pub to: T,
    /// How strongly the evidence points to it.
    pub confidence: Confidence,
}

/// How a module uses a model.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ModelUse {
    /// `_inherit`, extending it or inheriting from it.
    Inherit,
    /// `_inherits`, delegating to it.
    Delegate,
    /// The comodel of one of its fields.
    Comodel(FieldName),
    /// The model of a record, `<function>` or `<delete>` in a data file.
    Record,
}

/// Where a field path is written.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PathUse {
    /// `related=` of the field.
    Related(FieldName),
    /// `@api.depends` of the method.
    Depends(String),
    /// `@api.onchange` of the method.
    Onchange(String),
    /// `@api.constrains` of the method.
    Constrains(String),
    /// A `<field>` of the view with this external id.
    View(String),
    /// A value the record with this external id sets in a data file.
    RecordValue(String),
}

/// What is wrong with a reference to core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// A module listed in `depends` cannot be installed in the target
    /// version.
    DependencyGone {
        /// The dependency.
        dependency: ModuleName,
        /// The module it was probably renamed or merged into.
        renamed: Option<Renamed<ModuleName>>,
    },
    /// A model the module uses does not exist.
    ModelGone {
        /// The model.
        model: ModelName,
        /// How the module uses it.
        how: ModelUse,
        /// The model it was probably renamed or merged into.
        renamed: Option<Renamed<ModelName>>,
    },
    /// A field path goes through a field that does not exist.
    FieldGone {
        /// The model the missing field was looked up on.
        model: ModelName,
        /// The missing field.
        field: FieldName,
        /// The whole path, as written.
        path: String,
        /// Where the path is written.
        how: PathUse,
        /// Where the field probably went: model and field.
        renamed: Option<Renamed<(ModelName, FieldName)>>,
    },
    /// A core field the module declares again is gone from core: the module's
    /// declaration now makes a field of its own, without core's behavior.
    RedeclaredFieldGone {
        /// The model.
        model: ModelName,
        /// The field.
        field: FieldName,
    },
    /// A core field the module declares again has another type in core.
    RedeclaredFieldType {
        /// The model.
        model: ModelName,
        /// The field.
        field: FieldName,
        /// Core's type in the source version.
        from: FieldType,
        /// Core's type in the target version.
        to: FieldType,
    },
    /// A core method the module overrides is gone from core.
    OverriddenMethodGone {
        /// The model.
        model: ModelName,
        /// The method.
        method: String,
        /// Whether the override calls `super()`, which now fails.
        calls_super: bool,
    },
    /// A core method the module overrides takes other parameters.
    OverriddenMethodParams {
        /// The model.
        model: ModelName,
        /// The method.
        method: String,
        /// Core's parameters in the source version.
        from: Vec<String>,
        /// Core's parameters in the target version.
        to: Vec<String>,
    },
    /// A core method the module overrides does something else: the logic it
    /// extends changed, or, without `super()`, the core code it replaces
    /// changed and the override may be an outdated copy.
    OverriddenMethodBody {
        /// The model.
        model: ModelName,
        /// The method.
        method: String,
        /// Whether the override calls `super()`.
        calls_super: bool,
    },
    /// A record the module refers to by external id does not exist.
    RecordGone {
        /// The external id, as `module.xmlid`.
        xmlid: String,
        /// How the module refers to it.
        how: RefKind,
    },
    /// A model the module defines also exists in core now.
    ModelNameTaken {
        /// The model.
        model: ModelName,
    },
    /// Odoo's naming rules give the class another model name in the target
    /// version.
    ModelNameChanges {
        /// The Python class.
        class: String,
        /// Its model name in the source version.
        from: ModelName,
        /// Its model name in the target version.
        to: ModelName,
    },
}

/// Checks `upgrading`, found in both `source` and `target`, for what the
/// upgrade from `source` to `target` breaks. Both sets of addons hold core
/// and the modules being upgraded; `company_countries` decides
/// country-specific `auto_install` modules, as in [`install`].
pub fn scan(
    source: &Addons,
    target: &Addons,
    upgrading: &[ModuleName],
    company_countries: &[String],
) -> Scan {
    let options = InstallOptions {
        company_countries,
        upgrading,
    };
    let full_source = install_with(source, upgrading, &options);
    let full_target = install_with(target, upgrading, &options);
    // Core is everything the upgraded modules install, without them.
    let core: Vec<ModuleName> = full_source
        .modules
        .iter()
        .map(|module| module.name.clone())
        .filter(|name| !upgrading.contains(name))
        .collect();
    let core_source = install(source, &core, company_countries);
    let core_target = install(target, &core, company_countries);
    let changes = pontix_diff::diff(&core_source, &core_target);

    let mut issues = check::check(&check::Versions {
        source: check::Side::new(&full_source, &core_source),
        target: check::Side::new(&full_target, &core_target),
        upgrading,
        changes: &changes,
    });
    issues.sort_by(|a, b| {
        (&a.module, &a.at.file, a.at.line).cmp(&(&b.module, &b.at.file, b.at.line))
    });
    Scan { issues, changes }
}

#[cfg(test)]
mod tests;
