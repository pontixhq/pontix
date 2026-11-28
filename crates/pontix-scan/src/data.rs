//! What the core changes do to data already in a database: columns and
//! tables dropped, selection keys removed, stored types changed. These apply
//! wherever the core module is installed, custom code or not.

use std::collections::BTreeMap;

use pontix_core::{Change, ChangeSet, Location, ModelBase, ModelName, ModuleName, Rename};
use pontix_index::Installation;
use pontix_index::registry::{self, ModelView};

use crate::{Evidence, Issue, Problem, Renamed, Since};

pub(crate) fn check(changes: &ChangeSet, core: &Installation<'_>) -> Vec<Issue> {
    let models = registry::models(core);
    let mut issues = Vec::new();
    for change in &changes.changes {
        let (model, before, after, problem) = match change {
            Change::FieldRemoved {
                model,
                field,
                storage,
                before,
            } if storage.holds_data() == Some(true) => {
                let renamed = changes.renames.iter().find_map(|rename| match rename {
                    Rename::Field {
                        model: from_model,
                        from,
                        to_model,
                        to,
                        confidence,
                        ..
                    } if from_model == model && from == field => Some(Renamed {
                        to: (to_model.clone(), Some(to.clone())),
                        confidence: *confidence,
                    }),
                    _ => None,
                });
                let problem = Problem::DataGone {
                    model: model.clone(),
                    field: Some(field.clone()),
                    storage: *storage,
                    renamed,
                };
                (model, before, None, problem)
            }
            Change::FieldStorageChanged {
                model,
                field,
                from,
                to,
                before,
                after,
            } if from.holds_data() == Some(true) && to.holds_data() == Some(false) => {
                let problem = Problem::DataGone {
                    model: model.clone(),
                    field: Some(field.clone()),
                    storage: *from,
                    renamed: None,
                };
                (model, before, Some(after), problem)
            }
            Change::ModelRemoved {
                model,
                had_table: Some(true),
                before,
            } => {
                let renamed = changes.renames.iter().find_map(|rename| match rename {
                    Rename::Model {
                        from,
                        to,
                        confidence,
                        ..
                    } if from == model => Some(Renamed {
                        to: (to.clone(), None),
                        confidence: *confidence,
                    }),
                    _ => None,
                });
                let problem = Problem::DataGone {
                    model: model.clone(),
                    field: None,
                    storage: pontix_core::Storage::Column,
                    renamed,
                };
                (model, before, None, problem)
            }
            Change::SelectionKeysRemoved {
                model,
                field,
                keys,
                storage,
                before,
                after,
            } if storage.holds_data() == Some(true) => {
                let problem = Problem::SelectionKeysGone {
                    model: model.clone(),
                    field: field.clone(),
                    keys: keys.clone(),
                };
                (model, before, Some(after), problem)
            }
            Change::FieldTypeChanged {
                model,
                field,
                from,
                to,
                storage,
                before,
                after,
            } if storage.holds_data() == Some(true) => {
                let problem = Problem::StoredTypeChanged {
                    model: model.clone(),
                    field: field.clone(),
                    from: from.clone(),
                    to: to.clone(),
                };
                (model, before, Some(after), problem)
            }
            _ => continue,
        };
        // Rows of a transient model are cleaned up by Odoo anyway.
        if transient(&models, model) {
            continue;
        }
        let Some(module) = owner(core, before) else {
            continue;
        };
        issues.push(Issue {
            module,
            at: before.clone(),
            problem,
            since: Since::Upgrade,
            evidence: Evidence {
                before: Some(before.clone()),
                after: after.cloned(),
                rule: None,
            },
        });
    }
    issues
}

fn transient(models: &BTreeMap<ModelName, ModelView<'_>>, model: &ModelName) -> bool {
    models
        .get(model)
        .is_some_and(|view| view.base == ModelBase::Transient)
}

/// The core module whose directory holds `location`.
fn owner(core: &Installation<'_>, location: &Location) -> Option<ModuleName> {
    core.modules
        .iter()
        .filter(|module| location.file.starts_with(&module.path))
        .max_by_key(|module| module.path.as_os_str().len())
        .map(|module| module.name.clone())
}
