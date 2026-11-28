//! The checks: each reference a module being upgraded makes to core, looked
//! up in both versions.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;

use pontix_core::resolve::{model_kind, model_name};
use pontix_core::{
    ApiDecorator, ChangeSet, DiagnosticKind, Element, FieldDef, FieldName, Location, MethodDef,
    ModelDef, ModelKind, ModelName, ModuleName, Record, RefKind, Rename, View, Written, XmlRef,
};
use pontix_index::registry::{self, MethodView, ModelView, Xmlid, qualified};
use pontix_index::views::{self, Located, Views};
use pontix_index::{Framework, Installation, Module};

use crate::path::{Scope, Walk, lookup, walk};

mod code;
use crate::{Evidence, Issue, ModelUse, PathUse, Problem, Renamed, Since};

/// Where Odoo 19 names a class's model: only a string `_inherit` becomes
/// `_name`, otherwise the name comes from the class name. Odoo 18 used a
/// one-item `_inherit` list as the name too.
const MODEL_NAME_RULE: &str = "odoo/orm/models.py, MetaModel (19.0) vs odoo/models.py (18.0)";

/// One version: the installation with the modules being upgraded, and the
/// core it installs on its own.
pub(crate) struct Side<'i, 'a> {
    full: &'i Installation<'a>,
    /// Every model, the upgraded modules' declarations included.
    models: BTreeMap<ModelName, ModelView<'a>>,
    /// Every core model.
    core: BTreeMap<ModelName, ModelView<'a>>,
    /// Models a core class defines rather than only extends.
    core_defined: BTreeSet<ModelName>,
    /// The modules whose classes define each model, rather than only
    /// extend it: a model only extended does not exist in Odoo.
    definers: BTreeMap<ModelName, BTreeSet<ModuleName>>,
    /// Every external id, the upgraded modules' records included.
    xmlids: BTreeMap<String, Xmlid<'a>>,
    /// The framework, empty when it was not read.
    framework: &'a Framework,
    /// Every view, the upgraded modules' included.
    views: Views<'a>,
}

impl<'i, 'a> Side<'i, 'a> {
    pub(crate) fn new(
        full: &'i Installation<'a>,
        core: &Installation<'a>,
        framework: &'a Framework,
    ) -> Self {
        Self {
            full,
            models: registry::models(full),
            core: registry::models(core),
            core_defined: defined(core),
            definers: definers(full),
            xmlids: registry::xmlids(full),
            framework,
            views: Views::new(full),
        }
    }
}

impl Side<'_, '_> {
    /// Whether a module in scope defines the model.
    fn defines(&self, model: &ModelName, scope: Scope<'_>) -> bool {
        self.definers
            .get(model)
            .is_some_and(|modules| modules.iter().any(|module| scope.has(module)))
    }
}

fn definers(installation: &Installation<'_>) -> BTreeMap<ModelName, BTreeSet<ModuleName>> {
    installation
        .models()
        .into_iter()
        .map(|(name, definitions)| {
            let modules = definitions
                .iter()
                .filter(|definition| definition.kind != ModelKind::Extension)
                .map(|definition| definition.module.name.clone())
                .collect();
            (name, modules)
        })
        .collect()
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

/// For each module being upgraded, the modules it installs in the source
/// and in the target version. `None` on a side stands for every module, as
/// when a dependency cannot be installed there.
pub(crate) type Scopes =
    BTreeMap<ModuleName, (Option<BTreeSet<ModuleName>>, Option<BTreeSet<ModuleName>>)>;

/// Both versions and what is checked between them.
pub(crate) struct Versions<'s, 'i, 'a> {
    pub(crate) source: Side<'i, 'a>,
    pub(crate) target: Side<'i, 'a>,
    pub(crate) upgrading: &'s [ModuleName],
    pub(crate) changes: &'s ChangeSet,
    /// What each module being upgraded can rely on.
    pub(crate) scopes: &'s Scopes,
}

pub(crate) fn check(versions: &Versions<'_, '_, '_>) -> Vec<Issue> {
    let mut issues = Vec::new();
    dependencies(versions, &mut issues);
    for module in &versions.source.full.modules {
        if !versions.upgrading.contains(&module.name) {
            continue;
        }
        // A module the target cannot find at all is reported on its manifest
        // alone: everything else about it would fail the same way.
        if !versions
            .target
            .full
            .modules
            .iter()
            .any(|found| found.name == module.name)
        {
            continue;
        }
        let scope = match versions.scopes.get(&module.name) {
            Some((source, target)) => (scope_of(source.as_ref()), scope_of(target.as_ref())),
            None => (Scope::All, Scope::All),
        };
        let mut checker = Checker {
            versions,
            module: &module.name,
            scope,
            issues: &mut issues,
        };
        for def in &module.models {
            checker.class(def);
        }
        checker.data(module);
        checker.code(module);
    }
    issues
}

/// `None` stands for every module.
fn scope_of(modules: Option<&BTreeSet<ModuleName>>) -> Scope<'_> {
    modules.map_or(Scope::All, Scope::Modules)
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
                rule: None,
            },
        });
    }
}

/// One module being upgraded.
struct Checker<'c, 's, 'i, 'a> {
    versions: &'c Versions<'s, 'i, 'a>,
    module: &'c ModuleName,
    /// What the module can rely on in the source and in the target version.
    scope: (Scope<'c>, Scope<'c>),
    issues: &'c mut Vec<Issue>,
}

impl Checker<'_, '_, '_, '_> {
    /// A model class.
    fn class(&mut self, def: &ModelDef) {
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
                Evidence {
                    rule: Some(MODEL_NAME_RULE),
                    ..Evidence::default()
                },
            );
        }

        for parent in def.parents() {
            self.model(parent, ModelUse::Inherit, &def.location);
        }
        // Nothing else of a class can work on a model that is gone.
        if model_kind(def, source.full.version) == ModelKind::Extension
            && !target.defines(&name, self.scope.1)
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
                    rule: None,
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
            for found in &method.code_refs {
                self.code_ref(Some(&name), found, false);
            }
            self.overridden(&name, method);
        }
    }

    /// The module's data files and its `env.ref` calls.
    fn data(&mut self, module: &Module) {
        for (_, file) in &module.data {
            for (model, at) in &file.models {
                self.model(model, ModelUse::Record, at);
            }
            for record in &file.records {
                self.model(&record.model, ModelUse::Record, &record.location);
                let id = record
                    .id
                    .as_deref()
                    .map(|id| qualified(&module.name, id))
                    .unwrap_or_default();
                if self.versions.target.defines(&record.model, self.scope.1) {
                    self.record_fields(module, record, &id);
                }
                // After its fields, so that a locator whose field is gone is
                // reported once, as that field.
                if let Some(view) = &record.view {
                    self.view_targets(&id, view);
                }
            }
            for reference in &file.refs {
                self.xmlid(reference);
            }
        }
        for reference in &module.xmlid_refs {
            self.xmlid(reference);
        }
    }

    /// The fields a record sets and, for a view, those its architecture
    /// shows or locates.
    fn record_fields(&mut self, module: &Module, record: &Record, id: &str) {
        for (field, at) in &record.fields {
            let how = PathUse::RecordValue(id.to_string());
            self.path(&record.model, field.as_str(), how, at);
        }
        let Some(view) = &record.view else {
            return;
        };
        let Some(model) = self.view_model(&module.name, view) else {
            return;
        };
        for field in &view.fields {
            let path: Vec<&str> = field
                .within
                .iter()
                .chain(std::iter::once(&field.name))
                .map(FieldName::as_str)
                .collect();
            let how = PathUse::View(id.to_string());
            self.path(&model, &path.join("."), how, &field.location);
        }
    }

    /// The specs of an inheriting view, XPath or locator, that find
    /// something in the source version of the view it inherits and nothing
    /// in the target one. Each spec looks at the architecture the ones before it made, in
    /// the target version as far as they still apply. Whatever Pontix
    /// cannot combine or read with certainty is left out.
    fn view_targets(&mut self, id: &str, view: &View) {
        let (source, target) = (&self.versions.source, &self.versions.target);
        let (Some(own), Some(parent)) = (&view.arch, &view.inherit_id) else {
            return;
        };
        let (from, to) = self.scope;
        let in_source = |module: &ModuleName| from.has(module);
        let in_target = |module: &ModuleName| to.has(module);
        let Some(mut before) = source.views.before(id, &in_source) else {
            return;
        };
        let Some(mut after) = target.views.before(id, &in_target) else {
            return;
        };
        let parent = qualified(self.module, parent);
        let Some(parent_view) = target.views.get(&parent) else {
            return;
        };
        let parent_at = parent_view.location.clone();
        for spec in views::specs(own) {
            let Located::Found(found) = views::locate(&before, spec) else {
                return;
            };
            let worked = before.at(&found).map(|element| element.location.clone());
            // A `<field>` locator whose field is gone from the model is
            // reported once, as that field.
            let reported = spec.tag == "field"
                && self.issues.iter().any(|issue| {
                    issue.module == *self.module
                        && issue.at == spec.location
                        && matches!(
                            &issue.problem,
                            Problem::FieldGone { path, .. }
                                if Some(path.as_str()) == spec.attribute("name")
                        )
                });
            if !reported && views::locate(&after, spec) == Located::Missing {
                let xpath = spec.tag == "xpath";
                self.push(
                    &spec.location,
                    Problem::ViewTargetGone {
                        view: parent.clone(),
                        spec: if xpath {
                            spec.attribute("expr").unwrap_or_default().to_string()
                        } else {
                            locator(spec)
                        },
                        xpath,
                    },
                    Since::Upgrade,
                    Evidence {
                        before: worked,
                        after: Some(parent_at.clone()),
                        rule: None,
                    },
                );
            }
            if !views::apply_spec(&mut before, spec) {
                return;
            }
            // In the target, a spec that no longer applies is passed over.
            views::apply_spec(&mut after, spec);
        }
    }

    /// The model of a view: its own `model`, or that of the views it
    /// inherits, followed in the target version, then in the source one.
    fn view_model(&self, module: &ModuleName, view: &View) -> Option<ModelName> {
        if let Some(model) = &view.model {
            return Some(model.clone());
        }
        let mut parent = view.inherit_id.as_deref().map(|id| qualified(module, id));
        // Inheritance chains are short; a cycle must not loop forever.
        for _ in 0..32 {
            let id = parent?;
            let found = self
                .versions
                .target
                .xmlids
                .get(&id)
                .or_else(|| self.versions.source.xmlids.get(&id))?;
            let view = found.view?;
            if let Some(model) = &view.model {
                return Some(model.clone());
            }
            let owner = id
                .split_once('.')
                .map(|(owner, _)| ModuleName::new(owner))?;
            parent = view.inherit_id.as_deref().map(|id| qualified(&owner, id));
        }
        None
    }

    /// A reference to a record by external id.
    fn xmlid(&mut self, reference: &XmlRef) {
        let id = qualified(self.module, &reference.xmlid);
        let Some((owner, local)) = id.split_once('.') else {
            return;
        };
        // Records Odoo makes for modules and their categories are not core's
        // to break.
        if local.starts_with("module_") {
            return;
        }
        let (source, target) = (&self.versions.source, &self.versions.target);
        // Records of the modules being upgraded are theirs to keep, but
        // tests may still rely on the module's own demo data.
        if self
            .versions
            .upgrading
            .iter()
            .any(|module| module.as_str() == owner)
        {
            if let Some(record) = source.xmlids.get(&id) {
                let evidence = Evidence {
                    before: Some(record.location.clone()),
                    after: None,
                    rule: None,
                };
                self.demo_record(reference, record, id, evidence);
            }
            return;
        }
        // A record is only there for a module that installs its owner.
        let owner = ModuleName::new(owner);
        if let Some(record) = target.xmlids.get(&id)
            && self.scope.1.has(&owner)
        {
            let evidence = Evidence {
                before: None,
                after: Some(record.location.clone()),
                rule: None,
            };
            self.demo_record(reference, record, id, evidence);
            return;
        }
        let before = source
            .xmlids
            .get(&id)
            .filter(|_| self.scope.0.has(&owner))
            .map(|found| found.location.clone());
        let existed = before.is_some();
        // Tests refer to records Pontix cannot see, such as those of CSV
        // files: only one that is gone with the upgrade is worth a word.
        if !existed && reference.kind == RefKind::Test {
            return;
        }
        self.push(
            &reference.location,
            Problem::RecordGone {
                xmlid: id,
                how: reference.kind,
            },
            since(existed),
            Evidence {
                before,
                after: None,
                rule: None,
            },
        );
    }

    /// A record a test uses that only demo data creates, when the target
    /// makes databases without demo data by default.
    fn demo_record(
        &mut self,
        reference: &XmlRef,
        record: &Xmlid,
        xmlid: String,
        evidence: Evidence,
    ) {
        let changes = self.versions.changes;
        if reference.kind != RefKind::Test || !record.demo || changes.target.demo_by_default() {
            return;
        }
        self.push(
            &reference.location,
            Problem::DemoRecord { xmlid },
            since(changes.source.demo_by_default()),
            evidence,
        );
    }

    /// A model the class inherits, delegates to or points a field at.
    fn model(&mut self, model: &ModelName, how: ModelUse, at: &Location) {
        let (source, target) = (&self.versions.source, &self.versions.target);
        if target.defines(model, self.scope.1) {
            return;
        }
        let existed = source.defines(model, self.scope.0);
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
                rule: None,
            },
        );
    }

    /// A field path, followed from the class's model.
    fn path(&mut self, model: &ModelName, path: &str, how: PathUse, at: &Location) {
        let (source, target) = (&self.versions.source, &self.versions.target);
        // A model that is gone is reported where it is used; its fields would
        // only repeat that.
        if !target.defines(model, self.scope.1) {
            return;
        }
        let Walk::Missing { model: on, field } = walk(&target.models, self.scope.1, model, path)
        else {
            return;
        };
        let worked = walk(&source.models, self.scope.0, model, path) == Walk::Reaches;
        let existing = source
            .models
            .get(&on)
            .and_then(|view| lookup(view, &field, self.scope.0));
        // A field of a module being upgraded disappears only when its class
        // no longer applies, which is reported on the class.
        if existing.is_some_and(|found| self.versions.upgrading.contains(found.owner)) {
            return;
        }
        let before = existing.filter(|_| worked).map(|found| found.first.clone());
        let renamed = worked.then(|| self.field_renamed(&on, &field)).flatten();
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
                rule: None,
            },
        );
    }

    /// The field core most likely renamed `model.field` to.
    fn field_renamed(
        &self,
        model: &ModelName,
        field: &FieldName,
    ) -> Option<Renamed<(ModelName, FieldName)>> {
        self.versions
            .changes
            .renames
            .iter()
            .find_map(|rename| match rename {
                Rename::Field {
                    model: on,
                    from,
                    to_model,
                    to,
                    confidence,
                    ..
                } if on == model && from == field => Some(Renamed {
                    to: (to_model.clone(), to.clone()),
                    confidence: *confidence,
                }),
                _ => None,
            })
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
                    rule: None,
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
                    rule: None,
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
                    rule: None,
                },
            );
            return;
        };
        let evidence = Evidence {
            before: Some(before.first.location.clone()),
            after: Some(after.first.location.clone()),
            rule: None,
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

/// A locator spec as written, without its `position`: `<button
/// name="action_confirm">`.
fn locator(spec: &Element) -> String {
    let attributes: String = spec
        .attributes
        .iter()
        .filter(|(name, _)| name != "position")
        .map(|(name, value)| format!(" {name}=\"{value}\""))
        .collect();
    format!("<{}{attributes}>", spec.tag)
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
