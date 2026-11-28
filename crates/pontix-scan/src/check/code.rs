//! References the module's Python code makes: method and function bodies,
//! and tests.
//!
//! Python is not typed, so these checks are stricter than the ones on
//! declarations: a name is reported only when it was a field, or a model,
//! in the version upgraded from. Anything else may be a Python attribute
//! Pontix cannot see, and is left alone.

use pontix_core::{CodeRef, CodeRoot, Location, ModelName};
use pontix_index::Module;
use pontix_index::registry::qualified;

use super::Checker;
use crate::path::{Walk, lookup, walk};
use crate::{Evidence, ModelUse, PathUse, Problem, Since};

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
        let start = match &found.root {
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
        let how = if in_tests {
            PathUse::Test(found.how)
        } else {
            PathUse::Code(found.how)
        };
        self.code_path(&start, &found.path, how, &found.location);
    }

    /// `env["model"]`: reported only when the model existed before.
    fn code_model(&mut self, model: &ModelName, how: ModelUse, at: &Location) {
        if self.versions.source.defined.contains(model) {
            self.model(model, how, at);
        }
    }

    /// The first field of `path` that no longer exists, when it was a field
    /// on the model the same path reached in the version upgraded from.
    fn code_path(&mut self, start: &ModelName, path: &[String], how: PathUse, at: &Location) {
        let (source, target) = (&self.versions.source, &self.versions.target);
        // A model that is gone is reported where the code opens it.
        if !source.defined.contains(start) || !target.defined.contains(start) {
            return;
        }
        let mut prefix = String::new();
        for segment in path {
            if !prefix.is_empty() {
                prefix.push('.');
            }
            prefix.push_str(segment);
            let Walk::Missing { model: on, field } = walk(&target.models, start, &prefix) else {
                continue;
            };
            if walk(&source.models, start, &prefix) != Walk::Reaches {
                return;
            }
            let Some(found) = source.models.get(&on).and_then(|view| lookup(view, &field)) else {
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
