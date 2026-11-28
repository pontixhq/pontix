//! Which modules Odoo installs for a request, and the order it loads them.
//!
//! Installing a module installs its `depends`, then every `auto_install`
//! module whose triggers are all installed, until nothing changes
//! (`button_install` and `odoo/modules/db.py`). Modules then load by depth in
//! the dependency graph (`odoo/modules/graph.py` in 18,
//! `odoo/modules/module_graph.py` in 19).

use std::collections::{BTreeMap, HashMap};

use pontix_core::resolve::{model_kind, model_name};
use pontix_core::{
    Diagnostic, DiagnosticKind, ModelDef, ModelKind, ModelName, ModuleName, OdooVersion,
};

use crate::effective::{Effective, effective, effective_for};
use crate::files::whole_file;
use crate::scan::{Addons, Module};

/// The modules Odoo would install for a request, in load order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installation<'a> {
    /// The Odoo version whose rules decided.
    pub version: OdooVersion,
    /// Installed modules in the order Odoo loads them.
    pub modules: Vec<&'a Module>,
    /// Requested names that no addons path holds.
    pub unknown: Vec<ModuleName>,
    /// Why some modules could not be installed.
    pub diagnostics: Vec<Diagnostic>,
}

/// One class definition of a model, in an installed module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelRef<'a> {
    /// The module defining the class.
    pub module: &'a Module,
    /// The class.
    pub def: &'a ModelDef,
    /// Whether the class creates the model or extends it.
    pub kind: ModelKind,
}

impl<'a> Installation<'a> {
    /// Every model of the installation with its class definitions in load
    /// order, so later definitions extend earlier ones as in Odoo.
    pub fn models(&self) -> BTreeMap<ModelName, Vec<ModelRef<'a>>> {
        let mut models: BTreeMap<ModelName, Vec<ModelRef<'a>>> = BTreeMap::new();
        for &module in &self.modules {
            for def in &module.models {
                models
                    .entry(model_name(def, self.version).into_owned())
                    .or_default()
                    .push(ModelRef {
                        module,
                        def,
                        kind: model_kind(def, self.version),
                    });
            }
        }
        models
    }
}

/// How to install, beyond which modules: the countries of the companies,
/// which decide country-specific `auto_install` modules, and the modules
/// being upgraded or left out.
#[derive(Debug, Clone, Copy, Default)]
pub struct InstallOptions<'o> {
    /// Country codes of the companies, as in `res.country.code`.
    pub company_countries: &'o [String],
    /// Modules still written for another version, which Pontix checks for
    /// the upgrade: they are installed although their manifest version is of
    /// another series and although a dependency cannot be installed. That
    /// dependency is still reported on the module.
    pub upgrading: &'o [ModuleName],
    /// Modules never installed, not even by `auto_install`, so that neither
    /// they nor what depends on them is: core, without the custom modules
    /// that third-party `auto_install` bridges would pull in.
    pub left_out: &'o [ModuleName],
}

/// Installs `requested` the way Odoo would, on the modules `addons` found,
/// for a database whose companies are in `company_countries` (country codes
/// such as `id` or `fr`). Country-specific `auto_install` modules, like most
/// `l10n_*` bridges, are only added for those countries.
pub fn install<'a>(
    addons: &'a Addons,
    requested: &[ModuleName],
    company_countries: &[String],
) -> Installation<'a> {
    install_with(
        addons,
        requested,
        &InstallOptions {
            company_countries,
            ..InstallOptions::default()
        },
    )
}

/// [`install`], with the modules being upgraded installed as far as they go.
pub fn install_with<'a>(
    addons: &'a Addons,
    requested: &[ModuleName],
    options: &InstallOptions<'_>,
) -> Installation<'a> {
    let mut installer = Installer {
        version: addons.version,
        company_countries: options.company_countries,
        upgrading: options.upgrading,
        by_name: addons.modules.iter().map(|m| (m.name.clone(), m)).collect(),
        state: options
            .left_out
            .iter()
            .map(|name| (name.clone(), State::Failed))
            .collect(),
        installed: Vec::new(),
        diagnostics: Vec::new(),
    };
    let mut unknown = Vec::new();
    for name in requested {
        if installer.by_name.contains_key(name) {
            // A failure is already reported on the module itself.
            let _ = installer.add(name);
        } else {
            unknown.push(name.clone());
        }
    }
    installer.auto_install(addons);
    let modules = installer.load_order();
    Installation {
        version: addons.version,
        modules,
        unknown,
        diagnostics: installer.diagnostics,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    InProgress,
    Installed,
    Failed,
}

/// Why a dependency could not be installed.
#[derive(Debug, Clone, Copy)]
enum Refusal {
    Missing,
    NotInstallable,
    Cycle,
}

struct Installer<'a, 'c> {
    version: OdooVersion,
    company_countries: &'c [String],
    upgrading: &'c [ModuleName],
    by_name: HashMap<ModuleName, &'a Module>,
    state: HashMap<ModuleName, State>,
    installed: Vec<(&'a Module, Effective)>,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Installer<'a, '_> {
    /// Installs one module and its dependencies first. A module that fails
    /// is reported once, on its own manifest, with the cause.
    fn add(&mut self, name: &ModuleName) -> Result<(), Refusal> {
        match self.state.get(name) {
            Some(State::Installed) => return Ok(()),
            Some(State::Failed) => return Err(Refusal::NotInstallable),
            Some(State::InProgress) => return Err(Refusal::Cycle),
            None => {}
        }
        let Some(&module) = self.by_name.get(name) else {
            return Err(Refusal::Missing);
        };
        self.state.insert(name.clone(), State::InProgress);
        let outcome = self.add_module(module);
        let state = if outcome.is_ok() {
            State::Installed
        } else {
            State::Failed
        };
        self.state.insert(name.clone(), state);
        outcome
    }

    fn add_module(&mut self, module: &'a Module) -> Result<(), Refusal> {
        let upgrading = self.upgrading.contains(&module.name);
        let effective = match effective_for(module, self.version, upgrading) {
            Ok(effective) => effective,
            Err(diagnostic) => {
                self.diagnostics.push(diagnostic);
                return Err(Refusal::NotInstallable);
            }
        };
        for dependency in &effective.depends {
            if let Err(refusal) = self.add(dependency) {
                let kind = match refusal {
                    Refusal::Missing => DiagnosticKind::MissingDependency,
                    Refusal::NotInstallable => DiagnosticKind::DependencyNotInstallable,
                    Refusal::Cycle => DiagnosticKind::DependencyCycle,
                };
                self.diagnostics.push(Diagnostic {
                    snippet: dependency.to_string(),
                    ..whole_file(kind, &module.manifest_path)
                });
                // A module being upgraded is checked as far as it goes.
                if !upgrading {
                    return Err(Refusal::NotInstallable);
                }
            }
        }
        self.installed.push((module, effective));
        Ok(())
    }

    /// Adds `auto_install` modules whose triggers are all installed, until
    /// nothing changes. Candidates Odoo would skip (not installable, a
    /// dependency no addons path holds, or countries none of the companies
    /// are in) are skipped without a report.
    fn auto_install(&mut self, addons: &'a Addons) {
        loop {
            let mut added = false;
            for module in &addons.modules {
                if self.state.contains_key(&module.name) {
                    continue;
                }
                let Ok(effective) = effective(module, self.version) else {
                    continue;
                };
                let Some(triggers) = &effective.auto_install else {
                    continue;
                };
                let triggered = triggers
                    .iter()
                    .all(|trigger| self.state.get(trigger) == Some(&State::Installed));
                let available = effective
                    .depends
                    .iter()
                    .all(|dependency| self.by_name.contains_key(dependency));
                let countries = &module.manifest.countries;
                let for_these_companies = countries.is_empty()
                    || countries.iter().any(|country| {
                        self.company_countries
                            .iter()
                            .any(|company| company.eq_ignore_ascii_case(country))
                    });
                if triggered && available && for_these_companies && self.add(&module.name).is_ok() {
                    added = true;
                }
            }
            if !added {
                return;
            }
        }
    }

    /// `base` first, then by depth in the dependency graph, then by name. In
    /// Odoo 19 a `test_` module loads right after its deepest dependency.
    fn load_order(&self) -> Vec<&'a Module> {
        let effective: HashMap<&ModuleName, &Effective> = self
            .installed
            .iter()
            .map(|(module, effective)| (&module.name, effective))
            .collect();
        let mut keys: HashMap<&ModuleName, (usize, String)> = HashMap::new();
        // Dependencies come before dependents in `installed`, so every key a
        // module needs is computed before its own.
        for (module, effective) in &self.installed {
            let deepest = effective
                .depends
                .iter()
                .filter_map(|dependency| keys.get(dependency))
                .max()
                .cloned();
            let is_test = self.version.test_modules_follow_dependency()
                && module.name.as_str().starts_with("test_");
            let key = match deepest {
                Some((depth, order_name)) if is_test => {
                    (depth, format!("{order_name} {}", module.name))
                }
                Some((depth, _)) => (depth + 1, module.name.to_string()),
                None => (0, module.name.to_string()),
            };
            keys.insert(&module.name, key);
        }
        debug_assert_eq!(effective.len(), keys.len());
        let mut modules: Vec<&'a Module> =
            self.installed.iter().map(|(module, _)| *module).collect();
        modules.sort_by(|a, b| {
            let rank = |m: &Module| (m.name.as_str() != "base", keys.get(&m.name).cloned());
            rank(a).cmp(&rank(b))
        });
        modules
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;
    use std::path::PathBuf;

    use pontix_core::{AutoInstall, Inherit, Location, Manifest};

    use super::*;

    fn names(values: &[&str]) -> Vec<ModuleName> {
        values.iter().copied().map(ModuleName::new).collect()
    }

    fn module(name: &str, depends: &[&str], auto_install: AutoInstall) -> Module {
        Module {
            name: ModuleName::new(name),
            path: PathBuf::from(name),
            manifest_path: PathBuf::from(format!("{name}/__manifest__.py")),
            manifest: Manifest {
                depends: names(depends),
                auto_install,
                ..Manifest::default()
            },
            python_files: Vec::new(),
            models: Vec::new(),
            xmlid_refs: Vec::new(),
            code_refs: Vec::new(),
            test_refs: Vec::new(),
            data: Vec::new(),
        }
    }

    fn addons(version: OdooVersion, modules: Vec<Module>) -> Addons {
        Addons {
            version,
            modules,
            diagnostics: Vec::new(),
            framework: Default::default(),
        }
    }

    fn core(version: OdooVersion) -> Addons {
        addons(
            version,
            vec![
                module("base", &[], AutoInstall::WithAllDepends),
                module("sale", &["base"], AutoInstall::No),
                module("stock", &["base"], AutoInstall::No),
                module(
                    "sale_stock",
                    &["sale", "stock"],
                    AutoInstall::WithAllDepends,
                ),
                module("custom", &["sale"], AutoInstall::No),
            ],
        )
    }

    fn install_for<'a>(addons: &'a Addons, requested: &[ModuleName]) -> Installation<'a> {
        install(addons, requested, &[])
    }

    #[test]
    fn country_specific_auto_install_needs_a_company_there() {
        let mut modules = core(OdooVersion::V19).modules;
        modules.push(module(
            "l10n_id_sale",
            &["sale"],
            AutoInstall::WithAllDepends,
        ));
        modules.last_mut().expect("just pushed").manifest.countries = vec!["id".to_string()];
        let addons = addons(OdooVersion::V19, modules);
        let requested = names(&["sale"]);
        assert_eq!(order(&install(&addons, &requested, &[])), ["base", "sale"]);
        assert_eq!(
            order(&install(&addons, &requested, &["ID".to_string()])),
            ["base", "sale", "l10n_id_sale"]
        );
    }

    fn order<'a>(installation: &Installation<'a>) -> Vec<&'a str> {
        installation
            .modules
            .iter()
            .map(|m| m.name.as_str())
            .collect()
    }

    fn reported<'a>(installation: &'a Installation<'_>) -> Vec<(DiagnosticKind, &'a str)> {
        installation
            .diagnostics
            .iter()
            .map(|d| (d.kind, d.snippet.as_str()))
            .collect()
    }

    #[test]
    fn installs_dependencies_and_loads_by_depth() {
        let addons = core(OdooVersion::V19);
        let installation = install_for(&addons, &names(&["custom"]));
        assert_eq!(order(&installation), ["base", "sale", "custom"]);
        assert!(installation.diagnostics.is_empty());
    }

    #[test]
    fn auto_install_adds_bridge_modules_once_triggered() {
        let addons = core(OdooVersion::V19);
        let installation = install_for(&addons, &names(&["sale", "stock"]));
        assert_eq!(
            order(&installation),
            ["base", "sale", "stock", "sale_stock"]
        );
    }

    #[test]
    fn modules_left_out_are_never_installed() {
        let mut modules = core(OdooVersion::V19).modules;
        modules.push(module("needs_custom", &["custom"], AutoInstall::No));
        let addons = addons(OdooVersion::V19, modules);
        let left_out = names(&["sale_stock", "custom"]);
        let installation = install_with(
            &addons,
            &names(&["sale", "stock", "needs_custom"]),
            &InstallOptions {
                left_out: &left_out,
                ..InstallOptions::default()
            },
        );
        assert_eq!(order(&installation), ["base", "sale", "stock"]);
    }

    #[test]
    fn auto_install_triggers_can_be_a_subset_or_empty() {
        let mut modules = core(OdooVersion::V19).modules;
        modules.push(module(
            "sale_extra",
            &["sale", "stock"],
            AutoInstall::When(names(&["sale"])),
        ));
        modules.push(module("always", &["base"], AutoInstall::When(Vec::new())));
        let addons = addons(OdooVersion::V19, modules);
        // `sale_extra` brings `stock`, which then triggers `sale_stock`.
        let installation = install_for(&addons, &names(&["sale"]));
        assert_eq!(
            order(&installation),
            [
                "base",
                "always",
                "sale",
                "stock",
                "sale_extra",
                "sale_stock"
            ]
        );
    }

    #[test]
    fn unknown_requests_and_broken_dependencies_are_reported() {
        let mut modules = core(OdooVersion::V19).modules;
        modules.push(module(
            "needs_missing",
            &["sale", "nowhere"],
            AutoInstall::No,
        ));
        modules.push(module("old", &["base"], AutoInstall::No));
        modules.last_mut().expect("just pushed").manifest.version = Some("18.0.1.0.0".to_string());
        modules.push(module("needs_old", &["old"], AutoInstall::No));
        let addons = addons(OdooVersion::V19, modules);
        let installation = install_for(&addons, &names(&["needs_missing", "needs_old", "ghost"]));
        assert_eq!(installation.unknown, names(&["ghost"]));
        assert_eq!(order(&installation), ["base", "sale"]);
        assert_eq!(
            reported(&installation),
            [
                (DiagnosticKind::MissingDependency, "nowhere"),
                (DiagnosticKind::OtherSeriesVersion, "18.0.1.0.0"),
                (DiagnosticKind::DependencyNotInstallable, "old"),
            ]
        );
        assert!(
            installation.diagnostics[2]
                .location
                .file
                .ends_with("needs_old/__manifest__.py")
        );
    }

    #[test]
    fn modules_being_upgraded_install_as_far_as_they_go() {
        let mut modules = core(OdooVersion::V19).modules;
        modules.push(module(
            "custom_sale",
            &["sale", "sale_legacy"],
            AutoInstall::No,
        ));
        modules.last_mut().expect("just pushed").manifest.version = Some("18.0.1.0.0".to_string());
        let addons = addons(OdooVersion::V19, modules);
        let requested = names(&["custom_sale"]);
        let refused = install_for(&addons, &requested);
        assert_eq!(order(&refused), ["base"]);

        let upgrading = install_with(
            &addons,
            &requested,
            &InstallOptions {
                upgrading: &requested,
                ..InstallOptions::default()
            },
        );
        assert_eq!(order(&upgrading), ["base", "sale", "custom_sale"]);
        assert_eq!(
            reported(&upgrading),
            [(DiagnosticKind::MissingDependency, "sale_legacy")]
        );
    }

    #[test]
    fn dependency_cycles_are_reported() {
        let addons = addons(
            OdooVersion::V19,
            vec![
                module("base", &[], AutoInstall::WithAllDepends),
                module("a", &["b"], AutoInstall::No),
                module("b", &["a"], AutoInstall::No),
            ],
        );
        let installation = install_for(&addons, &names(&["a"]));
        assert_eq!(order(&installation), ["base"]);
        assert_eq!(
            reported(&installation),
            [
                (DiagnosticKind::DependencyCycle, "a"),
                (DiagnosticKind::DependencyNotInstallable, "b"),
            ]
        );
    }

    #[test]
    fn test_modules_follow_their_dependency_in_19_only() {
        let mut modules = core(OdooVersion::V19).modules;
        modules.push(module("test_sale", &["sale"], AutoInstall::No));
        let v19 = addons(OdooVersion::V19, modules.clone());
        let v18 = addons(OdooVersion::V18, modules);
        let requested = names(&["test_sale", "stock"]);
        assert_eq!(
            order(&install_for(&v19, &requested)),
            ["base", "sale", "test_sale", "stock", "sale_stock"]
        );
        assert_eq!(
            order(&install_for(&v18, &requested)),
            ["base", "sale", "stock", "sale_stock", "test_sale"]
        );
    }

    #[test]
    fn empty_depends_load_at_depth_zero_in_18() {
        let addons = addons(
            OdooVersion::V18,
            vec![
                module("base", &[], AutoInstall::No),
                module("web", &["base"], AutoInstall::No),
                module("alone", &[], AutoInstall::No),
            ],
        );
        let installation = install_for(&addons, &names(&["web", "alone"]));
        assert_eq!(order(&installation), ["base", "alone", "web"]);
    }

    #[test]
    fn models_list_every_definition_in_load_order() {
        let class = |class_name: &str, name: Option<&str>, inherit: Inherit| ModelDef {
            class_name: class_name.to_string(),
            name: name.map(ModelName::new),
            inherit,
            inherits: Vec::new(),
            base: pontix_core::ModelBase::Model,
            auto: pontix_core::Written::Absent,
            table_query: false,
            fields: Vec::new(),
            methods: Vec::new(),
            location: Location::new("models.py", NonZeroU32::MIN),
        };
        let mut modules = core(OdooVersion::V19).modules;
        modules[1]
            .models
            .push(class("SaleOrder", Some("sale.order"), Inherit::Absent));
        modules[4].models.push(class(
            "SaleOrder",
            None,
            Inherit::Single(ModelName::new("sale.order")),
        ));
        let addons = addons(OdooVersion::V19, modules);
        let installation = install_for(&addons, &names(&["custom"]));
        let models = installation.models();
        let sale_order: Vec<(&str, ModelKind)> = models[&ModelName::new("sale.order")]
            .iter()
            .map(|r| (r.module.name.as_str(), r.kind))
            .collect();
        assert_eq!(
            sale_order,
            [("sale", ModelKind::New), ("custom", ModelKind::Extension)]
        );
    }
}
