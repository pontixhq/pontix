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
mod data;
mod findings;
mod manifest;
mod path;

pub use findings::{NOT_COVERED, Report, report};

use std::collections::BTreeSet;

use pontix_core::{
    ChangeSet, CodeUse, Confidence, DiagnosticKind, FieldName, FieldType, Location, ModelName,
    ModuleName, RefKind, Storage,
};
use pontix_index::{Addons, InstallOptions, install_with};

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
    /// The Odoo rule that differs between the versions, when the issue comes
    /// from a rule rather than from something core declares.
    pub rule: Option<&'static str>,
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
    /// `env["model"]` in the module's Python code.
    Code,
    /// `env["model"]` in the module's tests.
    Test,
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
    /// The module's Python code.
    Code(CodeUse),
    /// The module's tests.
    Test(CodeUse),
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
    /// A test uses a record only demo data creates, and the target makes
    /// databases without demo data unless told otherwise.
    DemoRecord {
        /// The external id, as `module.xmlid`.
        xmlid: String,
    },
    /// The manifest version is of another series than the target: Odoo
    /// treats the module as not installable, with only a log warning.
    ManifestVersion {
        /// The version as written.
        written: String,
    },
    /// A `migrations/` folder is for a version below the target series: its
    /// scripts never run again, neither on upgrade nor on a new install.
    StaleMigration {
        /// The folder name: the version it migrates to.
        version: String,
    },
    /// The module's manifest is `__openerp__.py`, which the target version
    /// no longer reads: the module disappears.
    ManifestFileIgnored,
    /// The manifest is not one literal expression: Odoo ignores the module.
    ManifestUnreadable,
    /// An import inside the module points to nothing: the module fails to
    /// load.
    ImportBroken {
        /// The import as written.
        import: String,
    },
    /// Core drops a column or table that held data: a field removed or no
    /// longer stored, or a model removed. Applies to every database where the
    /// core module is installed.
    DataGone {
        /// The model.
        model: ModelName,
        /// The field; `None` for the model's whole table.
        field: Option<FieldName>,
        /// Where the values lived.
        storage: Storage,
        /// What it was probably renamed to: model and field.
        renamed: Option<Renamed<(ModelName, Option<FieldName>)>>,
    },
    /// Core no longer accepts selection keys that stored rows may hold.
    SelectionKeysGone {
        /// The model.
        model: ModelName,
        /// The field.
        field: FieldName,
        /// The keys removed.
        keys: Vec<String>,
    },
    /// A stored core field changes type: its values must be converted.
    StoredTypeChanged {
        /// The model.
        model: ModelName,
        /// The field.
        field: FieldName,
        /// Type in the source version.
        from: FieldType,
        /// Type in the target version.
        to: FieldType,
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
    /// A framework name or model method the module uses is gone.
    ApiGone {
        /// The name, such as `odoo.tools.ormcache_context` or
        /// `BaseModel._where_calc`.
        api: String,
        /// How the module uses it.
        how: ApiUse,
    },
    /// A framework name or model method the module uses is deprecated in
    /// the target version.
    ApiDeprecated {
        /// The name, as in [`Problem::ApiGone`].
        api: String,
        /// How the module uses it.
        how: ApiUse,
    },
}

/// How a module uses a framework name or a method of a model.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ApiUse {
    /// Imports it.
    Import,
    /// Reaches it as an attribute of a module it imports.
    Attribute,
    /// Calls it on records of the model.
    Call(ModelName),
    /// Any of these, in the module's tests.
    Test(Box<ApiUse>),
}

/// Checks `upgrading`, found in both `source` and `target`, for what the
/// upgrade from `source` to `target` breaks. Both sets of addons hold core
/// and the modules being upgraded; `company_countries` decides
/// country-specific `auto_install` modules, as in [`pontix_index::install`].
pub fn scan(
    source: &Addons,
    target: &Addons,
    upgrading: &[ModuleName],
    company_countries: &[String],
) -> Scan {
    scan_with(
        source,
        target,
        upgrading,
        &ScanOptions {
            company_countries,
            ..ScanOptions::default()
        },
    )
}

/// What a scan needs beyond the addons and the modules being upgraded.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScanOptions<'o> {
    /// Country codes of the companies, as in [`InstallOptions`].
    pub company_countries: &'o [String],
    /// Third-party modules the upgraded ones depend on, such as OCA ones,
    /// found in both sets of addons in their own version. They count as
    /// core, but a field one of them still declares must not hide what core
    /// itself renamed or dropped.
    pub third_party: &'o [ModuleName],
}

/// [`scan`], with third-party dependencies.
pub fn scan_with(
    source: &Addons,
    target: &Addons,
    upgrading: &[ModuleName],
    scan_options: &ScanOptions<'_>,
) -> Scan {
    let company_countries = scan_options.company_countries;
    let options = InstallOptions {
        company_countries,
        upgrading,
        ..InstallOptions::default()
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
    // A third-party `auto_install` module can be one being upgraded: core
    // never installs those, nor what needs them.
    let core_options = InstallOptions {
        company_countries,
        left_out: upgrading,
        ..InstallOptions::default()
    };
    let core_source = install_with(source, &core, &core_options);
    let core_target = install_with(target, &core, &core_options);
    let mut changes = pontix_diff::diff(&core_source, &core_target);
    if !scan_options.third_party.is_empty() {
        // Core alone, without the third-party modules and what needs them:
        // its renames and removals come first.
        let left_out: Vec<ModuleName> = upgrading
            .iter()
            .chain(scan_options.third_party)
            .cloned()
            .collect();
        let core_options = InstallOptions {
            left_out: &left_out,
            ..core_options
        };
        let own: Vec<ModuleName> = core
            .iter()
            .filter(|name| !left_out.contains(name))
            .cloned()
            .collect();
        let alone = pontix_diff::diff(
            &install_with(source, &own, &core_options),
            &install_with(target, &own, &core_options),
        );
        changes = merged(alone, changes);
    }

    // Each module is checked against what it installs itself, not against
    // everything the modules being upgraded install together.
    let in_target: BTreeSet<&ModuleName> = full_target
        .modules
        .iter()
        .map(|module| &module.name)
        .collect();
    let scopes = upgrading
        .iter()
        .map(|module| {
            let (before, complete) = installs(source, module, &options);
            // Pontix cannot tell what a module whose dependencies are missing
            // already relies on: every module counts then.
            let before = complete.then_some(before);
            let after = before.as_ref().map(|before| {
                // A dependency that is gone is reported on the manifest; what
                // the module reached through it and is still there counts,
                // or everything behind it would be reported again.
                let (mut after, _) = installs(target, module, &options);
                after.extend(
                    before
                        .iter()
                        .filter(|name| in_target.contains(name))
                        .cloned(),
                );
                after
            });
            (module.clone(), (before, after))
        })
        .collect();
    let mut issues = check::check(&check::Versions {
        source: check::Side::new(&full_source, &core_source, &source.framework),
        target: check::Side::new(&full_target, &core_target, &target.framework),
        upgrading,
        changes: &changes,
        scopes: &scopes,
    });
    issues.extend(manifest::check(source, target, upgrading));
    issues.extend(data::check(&changes, &core_source));
    issues.sort_by(|a, b| {
        (&a.module, &a.at.file, a.at.line).cmp(&(&b.module, &b.at.file, b.at.line))
    });
    Scan { issues, changes }
}

/// The modules `module` installs on its own, and whether all of its
/// dependencies could be installed.
fn installs(
    addons: &Addons,
    module: &ModuleName,
    options: &InstallOptions<'_>,
) -> (BTreeSet<ModuleName>, bool) {
    let installation = install_with(addons, std::slice::from_ref(module), options);
    let incomplete = !installation.unknown.is_empty()
        || installation.diagnostics.iter().any(|diagnostic| {
            matches!(
                diagnostic.kind,
                DiagnosticKind::MissingDependency | DiagnosticKind::DependencyNotInstallable
            )
        });
    let modules = installation
        .modules
        .iter()
        .map(|installed| installed.name.clone())
        .collect();
    (modules, !incomplete)
}

/// The changes of core alone, then those the third-party modules add.
fn merged(alone: ChangeSet, all: ChangeSet) -> ChangeSet {
    let mut changes = alone;
    for change in all.changes {
        if !changes.changes.contains(&change) {
            changes.changes.push(change);
        }
    }
    for rename in all.renames {
        if !changes.renames.contains(&rename) {
            changes.renames.push(rename);
        }
    }
    changes
}

#[cfg(test)]
mod tests;
