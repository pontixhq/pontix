//! References the module's Python code makes: method and function bodies,
//! and tests.
//!
//! Python is not typed, so these checks are stricter than the ones on
//! declarations: a name is reported only when it was a field, or a model,
//! in the version upgraded from. Anything else may be a Python attribute
//! Pontix cannot see, and is left alone.

use pontix_core::{CodeRef, CodeRoot, CodeUse, Location, ModelName};
use pontix_index::registry::qualified;
use pontix_index::{Lookup, Module};

use super::{Checker, find_method};
use crate::path::{Walk, lookup, reach, walk};
use crate::{ApiUse, Evidence, ModelUse, PathUse, Problem, Since};

impl Checker<'_, '_, '_, '_> {
    /// What the module's functions, non-model classes and tests use.
    pub(super) fn code(&mut self, module: &Module) {
        for found in &module.code_refs {
            self.code_ref(None, found, false);
        }
        for found in &module.test_refs {
            self.code_ref(None, found, true);
        }
    }

    /// One reference. `own` is the model whose method it is in, for
    /// references that start at `self`.
    pub(super) fn code_ref(&mut self, own: Option<&ModelName>, found: &CodeRef, in_tests: bool) {
        let tested = |how: ApiUse| {
            if in_tests {
                ApiUse::Test(Box::new(how))
            } else {
                how
            }
        };
        let start = match &found.root {
            CodeRoot::Framework(name) => {
                let how = match found.how {
                    CodeUse::Import => ApiUse::Import,
                    _ => ApiUse::Attribute,
                };
                return self.framework_name(name, tested(how), &found.location);
            }
            CodeRoot::Own => match own {
                Some(model) => model.clone(),
                None => return,
            },
            CodeRoot::Model(model) if found.path.is_empty() => {
                let how = if in_tests {
                    ModelUse::Test
                } else {
                    ModelUse::Code
                };
                return self.code_model(model, how, &found.location);
            }
            CodeRoot::Model(model) => model.clone(),
            CodeRoot::Xmlid(xmlid) => {
                let id = qualified(self.module, xmlid);
                match self.versions.source.xmlids.get(&id) {
                    Some(record) => record.model.clone(),
                    None => return,
                }
            }
        };
        if found.how == CodeUse::Call {
            let Some((method, path)) = found.path.split_last() else {
                return;
            };
            let Some(model) = reach(&self.versions.source.models, self.scope.0, &start, path)
            else {
                return;
            };
            let how = tested(ApiUse::Call(model.clone()));
            return self.method_call(&model, method, how, &found.location);
        }
        let how = if in_tests {
            PathUse::Test(found.how)
        } else {
            PathUse::Code(found.how)
        };
        self.code_path(&start, &found.path, how, &found.location);
    }

    /// A framework name, such as `odoo.osv.expression.AND`: each part is
    /// looked up in turn, and only one that existed before is reported.
    fn framework_name(&mut self, name: &str, how: ApiUse, at: &Location) {
        let (source, target) = (
            self.versions.source.framework,
            self.versions.target.framework,
        );
        if source.is_empty() || target.is_empty() {
            return;
        }
        let parts: Vec<&str> = name.split('.').collect();
        for end in 2..=parts.len() {
            let prefix = parts[..end].join(".");
            let Lookup::Found(before) = source.lookup(&prefix) else {
                return;
            };
            let problem = match target.lookup(&prefix) {
                Lookup::Missing => Problem::ApiGone { api: prefix, how },
                Lookup::Found(after)
                    if after.deprecated.is_some() && before.deprecated.is_none() =>
                {
                    let evidence = Evidence {
                        before: Some(before.location),
                        after: after.deprecated,
                        rule: None,
                    };
                    let problem = Problem::ApiDeprecated { api: prefix, how };
                    return self.push(at, problem, Since::Upgrade, evidence);
                }
                // Attributes of a class or a function are not followed.
                Lookup::Found(after) if after.module && before.module => continue,
                Lookup::Found(_) | Lookup::Unknown => return,
            };
            let evidence = Evidence {
                before: Some(before.location),
                after: None,
                rule: None,
            };
            return self.push(at, problem, Since::Upgrade, evidence);
        }
    }

    /// `records.method(...)` on records of `model`: reported when the method
    /// existed before, on the model or on every model through `BaseModel`.
    fn method_call(&mut self, model: &ModelName, method: &str, how: ApiUse, at: &Location) {
        let (source, target) = (&self.versions.source, &self.versions.target);
        let (Some(before), Some(after)) = (source.models.get(model), target.models.get(model))
        else {
            return;
        };
        if after.has_method(method) || target.framework.is_empty() {
            return;
        }
        let own = find_method(&source.models, model, method);
        // A method only the modules being upgraded define is theirs.
        if own.is_some_and(|found| {
            found
                .by_module
                .keys()
                .all(|module| self.versions.upgrading.contains(module))
        }) {
            return;
        }
        let base_before = source.framework.base_model(method);
        let defined_before = before.has_method(method) && own.is_some();
        if !defined_before && base_before.is_none() {
            return;
        }
        let location_before = own
            .map(|found| found.first.location.clone())
            .or_else(|| base_before.map(|api| api.location.clone()));
        let (problem, after_location) = match target.framework.base_model(method) {
            None if defined_before => (
                Problem::ApiGone {
                    api: format!("{model}.{method}"),
                    how,
                },
                None,
            ),
            None => (
                Problem::ApiGone {
                    api: format!("BaseModel.{method}"),
                    how,
                },
                None,
            ),
            Some(api)
                if !defined_before
                    && api.deprecated.is_some()
                    && base_before.is_some_and(|base| base.deprecated.is_none()) =>
            {
                (
                    Problem::ApiDeprecated {
                        api: format!("BaseModel.{method}"),
                        how,
                    },
                    api.deprecated.clone(),
                )
            }
            Some(_) => return,
        };
        let evidence = Evidence {
            before: location_before,
            after: after_location,
            rule: None,
        };
        self.push(at, problem, Since::Upgrade, evidence);
    }

    /// `env["model"]`: reported only when the model existed before.
    fn code_model(&mut self, model: &ModelName, how: ModelUse, at: &Location) {
        if self.versions.source.defines(model, self.scope.0) {
            self.model(model, how, at);
        }
    }

    /// The first field of `path` that no longer exists, when it was a field
    /// on the model the same path reached in the version upgraded from.
    fn code_path(&mut self, start: &ModelName, path: &[String], how: PathUse, at: &Location) {
        let (source, target) = (&self.versions.source, &self.versions.target);
        // A model that is gone is reported where the code opens it.
        if !source.defines(start, self.scope.0) || !target.defines(start, self.scope.1) {
            return;
        }
        let mut prefix = String::new();
        for segment in path {
            if !prefix.is_empty() {
                prefix.push('.');
            }
            prefix.push_str(segment);
            let Walk::Missing { model: on, field } =
                walk(&target.models, self.scope.1, start, &prefix)
            else {
                continue;
            };
            if walk(&source.models, self.scope.0, start, &prefix) != Walk::Reaches {
                return;
            }
            let Some(found) = source
                .models
                .get(&on)
                .and_then(|view| lookup(view, &field, self.scope.0))
            else {
                return;
            };
            // A field of a module being upgraded disappears only when its
            // class no longer applies, which is reported on the class.
            if self.versions.upgrading.contains(found.owner) {
                return;
            }
            let before = found.first.clone();
            let renamed = self.field_renamed(&on, &field);
            return self.push(
                at,
                Problem::FieldGone {
                    model: on,
                    field,
                    path: prefix,
                    how,
                    renamed,
                },
                Since::Upgrade,
                Evidence {
                    before: Some(before),
                    after: None,
                    rule: None,
                },
            );
        }
    }
}
