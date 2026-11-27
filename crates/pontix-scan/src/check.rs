//! The checks: each reference a module being upgraded makes to core, looked
//! up in both versions.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;

use pontix_core::resolve::{model_kind, model_name};
use pontix_core::{
    ApiDecorator, ChangeSet, DiagnosticKind, FieldDef, Location, MethodDef, ModelDef, ModelKind,
    ModelName, ModuleName, Rename, Written,
};
use pontix_index::Installation;
use pontix_index::registry::{self, MethodView, ModelView};

use crate::path::{Walk, lookup, walk};
use crate::{Evidence, Issue, ModelUse, PathUse, Problem, Renamed, Since};

/// One version: the installation with the modules being upgraded, and the
/// core it installs on its own.
pub(crate) struct Side<'i, 'a> {
    full: &'i Installation<'a>,
    /// Every model, the upgraded modules' declarations included.
    models: BTreeMap<ModelName, ModelView<'a>>,
    /// Every core model.
    core: BTreeMap<ModelName, ModelView<'a>>,
    /// Models some class defines rather than only extends: a model only
    /// extended does not exist in Odoo.
    defined: BTreeSet<ModelName>,
    /// The same, for core alone.
    core_defined: BTreeSet<ModelName>,
}

impl<'i, 'a> Side<'i, 'a> {
    pub(crate) fn new(full: &'i Installation<'a>, core: &Installation<'a>) -> Self {
        Self {
            full,
            models: registry::models(full),
            core: registry::models(core),
            defined: defined(full),
            core_defined: defined(core),
        }
    }
}

fn defined(installation: &Installation<'_>) -> BTreeSet<ModelName> {
    installation
        .models()
        .into_iter()
        .filter(|(_, definitions)| {
            definitions
                .iter()
                .any(|definition| definition.kind != ModelKind::Extension)
        })
        .map(|(name, _)| name)
        .collect()
}

/// Both versions and what is checked between them.
pub(crate) struct Versions<'s, 'i, 'a> {
    pub(crate) source: Side<'i, 'a>,
    pub(crate) target: Side<'i, 'a>,
    pub(crate) upgrading: &'s [ModuleName],
    pub(crate) changes: &'s ChangeSet,
}

pub(crate) fn check(versions: &Versions<'_, '_, '_>) -> Vec<Issue> {
    let mut issues = Vec::new();
    dependencies(versions, &mut issues);
    for module in &versions.source.full.modules {
        if !versions.upgrading.contains(&module.name) {
            continue;
        }
        for def in &module.models {
            let mut class = Class {
                versions,
                module: &module.name,
                issues: &mut issues,
            };
            class.check(def);
        }
    }
    issues
}

/// Dependencies the target version cannot install.
fn dependencies(versions: &Versions<'_, '_, '_>, issues: &mut Vec<Issue>) {
    let refused = |diagnostic: &pontix_core::Diagnostic| {
        matches!(
            diagnostic.kind,
            DiagnosticKind::MissingDependency | DiagnosticKind::DependencyNotInstallable
        )
    };
    for diagnostic in versions
        .target
        .full
        .diagnostics
        .iter()
        .filter(|d| refused(d))
    {
        let Some(module) = versions
            .target
            .full
            .modules
            .iter()
            .find(|module| module.manifest_path == diagnostic.location.file)
            .filter(|module| versions.upgrading.contains(&module.name))
        else {
            continue;
        };
        let dependency = ModuleName::new(diagnostic.snippet.as_str());
        let before = versions
            .source
            .full
            .modules
            .iter()
            .find(|module| module.name == dependency)
            .map(|module| Location::new(&module.manifest_path, NonZeroU32::MIN));
        let since = if before.is_some() {
            Since::Upgrade
        } else {
            Since::Before
        };
        let renamed = versions
            .changes
            .renames
            .iter()
            .find_map(|rename| match rename {
                Rename::Module {
                    from,
                    to,
                    confidence,
                    ..
                } if *from == dependency => Some(Renamed {
                    to: to.clone(),
                    confidence: *confidence,
                }),
                _ => None,
            });
        issues.push(Issue {
            module: module.name.clone(),
            at: diagnostic.location.clone(),
            problem: Problem::DependencyGone {
                dependency,
                renamed,
            },
            since,
            evidence: Evidence {
                before,
                after: None,
            },
        });
    }
}

/// One class of a module being upgraded.
struct Class<'c, 's, 'i, 'a> {
    versions: &'c Versions<'s, 'i, 'a>,
    module: &'c ModuleName,
    issues: &'c mut Vec<Issue>,
}

impl Class<'_, '_, '_, '_> {
    fn check(&mut self, def: &ModelDef) {
        let (source, target) = (&self.versions.source, &self.versions.target);
        let name = model_name(def, source.full.version).into_owned();
        let renamed_to = model_name(def, target.full.version).into_owned();
        if renamed_to != name {
            self.push(
                &def.location,
                Problem::ModelNameChanges {
                    class: def.class_name.clone(),
                    from: name.clone(),
                    to: renamed_to,
                },
                Since::Upgrade,
                Evidence::default(),
            );
        }

        for parent in def.parents() {
            self.model(parent, ModelUse::Inherit, &def.location);
        }
        // Nothing else of a class can work on a model that is gone.
        if model_kind(def, source.full.version) == ModelKind::Extension
            && !target.defined.contains(&name)
        {
            return;
        }
        for delegation in &def.inherits {
            self.model(&delegation.parent, ModelUse::Delegate, &def.location);
        }
        if model_kind(def, source.full.version) != ModelKind::Extension
            && !source.core_defined.contains(&name)
            && target.core_defined.contains(&name)
        {
            let after = target.core.get(&name).map(|view| view.first.clone());
            self.push(
                &def.location,
                Problem::ModelNameTaken {
                    model: name.clone(),
                },
                Since::Upgrade,
                Evidence {
                    before: None,
                    after,
                },
            );
        }

        for field in &def.fields {
            if let Some(comodel) = &field.comodel {
                self.model(
                    comodel,
                    ModelUse::Comodel(field.name.clone()),
                    &field.location,
                );
            }
            if let Written::Value(path) = &field.attrs.related {
                self.path(
                    &name,
                    path,
                    PathUse::Related(field.name.clone()),
                    &field.location,
                );
            }
            self.redeclared(&name, field);
        }
        for method in &def.methods {
            for paths in &method.field_paths {
                let how = |method: String| match paths.decorator {
                    ApiDecorator::Depends => PathUse::Depends(method),
                    ApiDecorator::Onchange => PathUse::Onchange(method),
                    ApiDecorator::Constrains => PathUse::Constrains(method),
                };
                for path in &paths.paths {
                    self.path(&name, path, how(method.name.clone()), &paths.location);
                }
            }
            self.overridden(&name, method);
        }
    }

    /// A model the class inherits, delegates to or points a field at.
    fn model(&mut self, model: &ModelName, how: ModelUse, at: &Location) {
        let (source, target) = (&self.versions.source, &self.versions.target);
        if target.defined.contains(model) {
            return;
        }
        let existed = source.defined.contains(model);
        let renamed = existed
            .then(|| {
                self.versions
                    .changes
                    .renames
                    .iter()
                    .find_map(|rename| match rename {
                        Rename::Model {
                            from,
                            to,
                            confidence,
                            ..
                        } if from == model => Some(Renamed {
                            to: to.clone(),
                            confidence: *confidence,
                        }),
                        _ => None,
                    })
            })
            .flatten();
        let before = source
            .core
            .get(model)
            .or_else(|| source.models.get(model))
            .filter(|_| existed)
            .map(|view| view.first.clone());
        self.push(
            at,
            Problem::ModelGone {
                model: model.clone(),
                how,
                renamed,
            },
            since(existed),
            Evidence {
                before,
                after: None,
            },
        );
    }

    /// A field path, followed from the class's model.
    fn path(&mut self, model: &ModelName, path: &str, how: PathUse, at: &Location) {
        let (source, target) = (&self.versions.source, &self.versions.target);
        let Walk::Missing { model: on, field } = walk(&target.models, model, path) else {
            return;
        };
        let worked = walk(&source.models, model, path) == Walk::Reaches;
        let before = source
            .models
            .get(&on)
            .and_then(|view| lookup(view, &field))
            .filter(|_| worked)
            .map(|found| found.first.clone());
        let renamed = worked
            .then(|| {
                self.versions
                    .changes
                    .renames
                    .iter()
                    .find_map(|rename| match rename {
                        Rename::Field {
                            model,
                            from,
                            to_model,
                            to,
                            confidence,
                            ..
                        } if *model == on && *from == field => Some(Renamed {
                            to: (to_model.clone(), to.clone()),
                            confidence: *confidence,
                        }),
                        _ => None,
                    })
            })
            .flatten();
        self.push(
            at,
            Problem::FieldGone {
                model: on,
                field,
                path: path.to_string(),
                how,
                renamed,
            },
            since(worked),
            Evidence {
                before,
                after: None,
            },
        );
    }

    /// A field the class declares that core declares too.
    fn redeclared(&mut self, model: &ModelName, field: &FieldDef) {
        let (source, target) = (&self.versions.source, &self.versions.target);
        let Some(before) = source
            .core
            .get(model)
            .and_then(|view| view.field(&field.name))
        else {
            return;
        };
        let Some(core) = target.core.get(model) else {
            return;
        };
        match core.field(&field.name) {
            None if !core.delegated_fields.contains_key(&field.name) => self.push(
                &field.location,
                Problem::RedeclaredFieldGone {
                    model: model.clone(),
                    field: field.name.clone(),
                },
                Since::Upgrade,
                Evidence {
                    before: Some(before.first.clone()),
                    after: None,
                },
            ),
            Some(after) if after.field_type != before.field_type => self.push(
                &field.location,
                Problem::RedeclaredFieldType {
                    model: model.clone(),
                    field: field.name.clone(),
                    from: before.field_type.clone(),
                    to: after.field_type.clone(),
                },
                Since::Upgrade,
                Evidence {
                    before: Some(before.type_at.clone()),
                    after: Some(after.type_at.clone()),
                },
            ),
            _ => {}
        }
    }

    /// A method the class defines that core defines on the model, or on a
    /// model it inherits from.
    fn overridden(&mut self, model: &ModelName, method: &MethodDef) {
        let (source, target) = (&self.versions.source, &self.versions.target);
        let Some(before) = find_method(&source.core, model, &method.name) else {
            return;
        };
        if !target.core.contains_key(model) {
            return;
        }
        let Some(after) = find_method(&target.core, model, &method.name) else {
            self.push(
                &method.location,
                Problem::OverriddenMethodGone {
                    model: model.clone(),
                    method: method.name.clone(),
                    calls_super: method.calls_super,
                },
                Since::Upgrade,
                Evidence {
                    before: Some(before.first.location.clone()),
                    after: None,
                },
            );
            return;
        };
        let evidence = Evidence {
            before: Some(before.first.location.clone()),
            after: Some(after.first.location.clone()),
        };
        if before.first.params != after.first.params {
            self.push(
                &method.location,
                Problem::OverriddenMethodParams {
                    model: model.clone(),
                    method: method.name.clone(),
                    from: before.first.params.clone(),
                    to: after.first.params.clone(),
                },
                Since::Upgrade,
                evidence.clone(),
            );
        }
        if fingerprints(before) != fingerprints(after) {
            self.push(
                &method.location,
                Problem::OverriddenMethodBody {
                    model: model.clone(),
                    method: method.name.clone(),
                    calls_super: method.calls_super,
                },
                Since::Upgrade,
                evidence,
            );
        }
    }

    fn push(&mut self, at: &Location, problem: Problem, since: Since, evidence: Evidence) {
        self.issues.push(Issue {
            module: self.module.clone(),
            at: at.clone(),
            problem,
            since,
            evidence,
        });
    }
}

fn since(worked_before: bool) -> Since {
    if worked_before {
        Since::Upgrade
    } else {
        Since::Before
    }
}

/// The method as the model has it: its own, or that of the nearest model it
/// inherits from.
fn find_method<'v, 'a>(
    models: &'v BTreeMap<ModelName, ModelView<'a>>,
    model: &ModelName,
    method: &str,
) -> Option<&'v MethodView<'a>> {
    let mut seen = BTreeSet::new();
    let mut pending = vec![model.clone()];
    while let Some(name) = pending.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let Some(view) = models.get(&name) else {
            continue;
        };
        if let Some(found) = view.methods.get(method) {
            return Some(found);
        }
        pending.extend(view.parents.iter().rev().cloned());
    }
    None
}

/// What each module's definition of the method does.
fn fingerprints<'a>(method: &MethodView<'a>) -> BTreeMap<&'a ModuleName, u64> {
    method
        .by_module
        .iter()
        .map(|(module, def)| (*module, def.fingerprint))
        .collect()
}
