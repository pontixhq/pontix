//! Compares two Odoo versions, each as an installation of its core modules,
//! and lists what changed between them as a [`ChangeSet`].
//!
//! Models are compared as a whole: a model's fields and methods are those of
//! every module that defines or extends it. Which module introduces a field
//! is kept, because a field that moves to another module disappears for
//! custom code that does not depend on that module.
#![forbid(unsafe_code)]

mod rename;

use std::collections::BTreeSet;
use std::num::NonZeroU32;
use std::path::Path;

use pontix_core::{Change, ChangeSet, FieldName, Location, ModelName, Storage, Written};
use pontix_index::Installation;

use pontix_index::registry::{FieldView, MethodView, ModelView, models};

/// Every change between `source` and `target`, ordered by module, then by
/// model, then by field and method name.
pub fn diff(source: &Installation<'_>, target: &Installation<'_>) -> ChangeSet {
    let mut changes = Vec::new();

    let target_modules: BTreeSet<_> = target.modules.iter().map(|m| &m.name).collect();
    for module in &source.modules {
        if !target_modules.contains(&module.name) {
            changes.push(Change::ModuleRemoved {
                module: module.name.clone(),
                before: manifest_location(&module.manifest_path),
            });
        }
    }

    let before = models(source);
    let after = models(target);
    let names: BTreeSet<&ModelName> = before.keys().chain(after.keys()).collect();
    for name in names {
        match (before.get(name), after.get(name)) {
            (Some(old), Some(new)) => diff_model(name, old, new, &mut changes),
            (Some(old), None) => changes.push(Change::ModelRemoved {
                model: name.clone(),
                had_table: old.has_table,
                before: old.first.clone(),
            }),
            (None, Some(new)) => changes.push(Change::ModelAdded {
                model: name.clone(),
                after: new.first.clone(),
            }),
            (None, None) => {}
        }
    }

    let source_modules: BTreeSet<_> = source.modules.iter().map(|m| &m.name).collect();
    let renames = rename::renames(&changes, &before, &after, |module| {
        source_modules.contains(module)
    });
    ChangeSet {
        source: source.version,
        target: target.version,
        changes,
        renames,
    }
}

fn diff_model(
    model: &ModelName,
    old: &ModelView<'_>,
    new: &ModelView<'_>,
    changes: &mut Vec<Change>,
) {
    // A mixin brings many methods at once, so a parent that comes or goes is
    // one change rather than one per method.
    for parent in new.parents.difference(&old.parents) {
        changes.push(Change::ParentAdded {
            model: model.clone(),
            parent: parent.clone(),
            after: new.first.clone(),
        });
    }
    for parent in old.parents.difference(&new.parents) {
        changes.push(Change::ParentRemoved {
            model: model.clone(),
            parent: parent.clone(),
            before: old.first.clone(),
        });
    }

    let fields: BTreeSet<_> = old
        .fields
        .keys()
        .chain(old.inherited_fields.keys())
        .chain(new.fields.keys())
        .chain(new.inherited_fields.keys())
        .copied()
        .collect();
    for field in fields {
        // A field counts as there however the model reaches it: one that
        // moved into a mixin or a delegated model is not removed. Fields only
        // reached through delegation are reported on their own model.
        match (old.field(field), new.field(field)) {
            (Some(before), Some(after)) => {
                if let (Some(before), Some(after)) = (old.fields.get(field), new.fields.get(field))
                {
                    compare_field(model, field, before, after, changes);
                }
                compare_data(model, field, (old, before), (new, after), changes);
            }
            // Reached through delegation instead, or the other way round: the
            // values move between this model's table and the delegated one.
            (Some(before), None) if new.reaches_field(field) => {
                if let Some(after) = new.delegated_fields.get(field) {
                    let from = old.storage(before);
                    let place = (before.type_at, after.type_at);
                    storage_change(model, field, from, Storage::NotStored, place, changes);
                }
            }
            (None, Some(after)) if old.reaches_field(field) => {
                if let Some(before) = old.delegated_fields.get(field) {
                    let to = new.storage(after);
                    let place = (before.type_at, after.type_at);
                    storage_change(model, field, Storage::NotStored, to, place, changes);
                }
            }
            (Some(before), None) => changes.push(Change::FieldRemoved {
                model: model.clone(),
                field: field.clone(),
                storage: old.storage(before),
                before: before.first.clone(),
            }),
            (None, Some(after)) => changes.push(Change::FieldAdded {
                model: model.clone(),
                field: field.clone(),
                storage: new.storage(after),
                after: after.first.clone(),
            }),
            (None, None) => {}
        }
    }

    // Methods the model defines itself, in either version. One it only
    // inherits is not reported: its parent model reports it, and a parent
    // that comes or goes is reported above.
    let methods: BTreeSet<_> = old
        .methods
        .keys()
        .chain(new.methods.keys())
        .copied()
        .collect();
    for method in methods {
        match (old.methods.get(method), new.methods.get(method)) {
            (Some(before), Some(after)) => compare_method(model, method, before, after, changes),
            (Some(before), None) if !new.has_method(method) => {
                changes.push(Change::MethodRemoved {
                    model: model.clone(),
                    method: method.to_string(),
                    before: before.first.location.clone(),
                });
            }
            (None, Some(after)) if !old.has_method(method) => {
                changes.push(Change::MethodAdded {
                    model: model.clone(),
                    method: method.to_string(),
                    after: after.first.location.clone(),
                });
            }
            _ => {}
        }
    }
}

/// Compares where a field both versions have keeps its values, and the keys
/// a selection field accepts: what the rows already in the database rely on.
fn compare_data(
    model: &ModelName,
    field: &FieldName,
    (old, before): (&ModelView<'_>, &FieldView<'_>),
    (new, after): (&ModelView<'_>, &FieldView<'_>),
    changes: &mut Vec<Change>,
) {
    let (from, to) = (old.storage(before), new.storage(after));
    let place = (before.type_at, after.type_at);
    storage_change(model, field, from, to, place, changes);
    if let (Written::Value(was), Written::Value(now)) =
        (before.selection_keys(), after.selection_keys())
    {
        let keys: Vec<String> = was.into_iter().filter(|key| !now.contains(key)).collect();
        if !keys.is_empty() {
            changes.push(Change::SelectionKeysRemoved {
                model: model.clone(),
                field: field.clone(),
                keys,
                storage: from,
                before: before.type_at.clone(),
                after: after.type_at.clone(),
            });
        }
    }
}

/// Reports a field whose values move, when both places are known.
fn storage_change(
    model: &ModelName,
    field: &FieldName,
    from: Storage,
    to: Storage,
    (before, after): (&Location, &Location),
    changes: &mut Vec<Change>,
) {
    if from != to && from != Storage::Unknown && to != Storage::Unknown {
        changes.push(Change::FieldStorageChanged {
            model: model.clone(),
            field: field.clone(),
            from,
            to,
            before: before.clone(),
            after: after.clone(),
        });
    }
}

/// Compares a field both versions declare on the model itself.
fn compare_field(
    model: &ModelName,
    field: &FieldName,
    before: &FieldView<'_>,
    after: &FieldView<'_>,
    changes: &mut Vec<Change>,
) {
    if before.field_type != after.field_type {
        changes.push(Change::FieldTypeChanged {
            model: model.clone(),
            field: field.clone(),
            from: before.field_type.clone(),
            to: after.field_type.clone(),
            before: before.type_at.clone(),
            after: after.type_at.clone(),
        });
    }
    // An unnamed comodel is inherited from an earlier declaration, so only
    // two named ones can be compared.
    if let (Some((from, from_at)), Some((to, to_at))) = (before.comodel, after.comodel)
        && from != to
    {
        changes.push(Change::FieldComodelChanged {
            model: model.clone(),
            field: field.clone(),
            from: from.clone(),
            to: to.clone(),
            before: from_at.clone(),
            after: to_at.clone(),
        });
    }
    if before.owner != after.owner {
        changes.push(Change::FieldMoved {
            model: model.clone(),
            field: field.clone(),
            from: before.owner.clone(),
            to: after.owner.clone(),
            before: before.first.clone(),
            after: after.first.clone(),
        });
    }
}

/// Compares a method both versions define on the model itself.
fn compare_method(
    model: &ModelName,
    method: &str,
    before: &MethodView<'_>,
    after: &MethodView<'_>,
    changes: &mut Vec<Change>,
) {
    if before.first.params != after.first.params {
        changes.push(Change::MethodParamsChanged {
            model: model.clone(),
            method: method.to_string(),
            from: before.first.params.clone(),
            to: after.first.params.clone(),
            before: before.first.location.clone(),
            after: after.first.location.clone(),
        });
    }
    for (module, old_def) in &before.by_module {
        if let Some(new_def) = after.by_module.get(module)
            && old_def.fingerprint != new_def.fingerprint
        {
            changes.push(Change::MethodBodyChanged {
                model: model.clone(),
                method: method.to_string(),
                module: (*module).clone(),
                before: old_def.location.clone(),
                after: new_def.location.clone(),
            });
        }
    }
}

fn manifest_location(path: &Path) -> Location {
    Location::new(path, NonZeroU32::MIN)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pontix_core::{
        AutoInstall, Confidence, FieldAttrs, FieldDef, FieldName, FieldType, Inherit, Manifest,
        MethodDef, ModelBase, ModelDef, ModuleName, OdooVersion, Rename,
    };
    use pontix_index::{Addons, Module, install};

    use super::*;

    fn at(file: &str, line: u32) -> Location {
        Location::new(file, NonZeroU32::new(line).expect("lines start at 1"))
    }

    fn field(name: &str, field_type: FieldType, comodel: Option<&str>, line: u32) -> FieldDef {
        FieldDef {
            name: FieldName::new(name),
            field_type,
            comodel: comodel.map(ModelName::new),
            attrs: FieldAttrs::default(),
            location: at("models.py", line),
        }
    }

    fn method(name: &str, params: &[&str], fingerprint: u64, line: u32) -> MethodDef {
        MethodDef {
            name: name.to_string(),
            params: params.iter().map(|p| p.to_string()).collect(),
            calls_super: false,
            field_paths: Vec::new(),
            fingerprint,
            location: at("models.py", line),
        }
    }

    fn model(
        name: &str,
        inherit: Inherit,
        fields: Vec<FieldDef>,
        methods: Vec<MethodDef>,
    ) -> ModelDef {
        ModelDef {
            class_name: "Class".to_string(),
            name: (inherit == Inherit::Absent).then_some(ModelName::new(name)),
            inherit,
            inherits: Vec::new(),
            base: ModelBase::Model,
            auto: Written::Absent,
            table_query: false,
            fields,
            methods,
            location: at("models.py", 1),
        }
    }

    fn extends(name: &str) -> Inherit {
        Inherit::Single(ModelName::new(name))
    }

    fn module(name: &str, depends: &[&str], models: Vec<ModelDef>) -> Module {
        Module {
            name: ModuleName::new(name),
            path: PathBuf::from(name),
            manifest_path: PathBuf::from(format!("{name}/__manifest__.py")),
            manifest: Manifest {
                depends: depends.iter().copied().map(ModuleName::new).collect(),
                auto_install: if name == "base" {
                    AutoInstall::WithAllDepends
                } else {
                    AutoInstall::No
                },
                ..Manifest::default()
            },
            python_files: Vec::new(),
            models,
            xmlid_refs: Vec::new(),
            data: Vec::new(),
        }
    }

    fn changes(source: Vec<Module>, target: Vec<Module>) -> Vec<Change> {
        let all = |modules: &[Module]| -> Vec<ModuleName> {
            modules.iter().map(|m| m.name.clone()).collect()
        };
        let source = Addons {
            version: OdooVersion::V18,
            diagnostics: Vec::new(),
            modules: source,
        };
        let target = Addons {
            version: OdooVersion::V19,
            diagnostics: Vec::new(),
            modules: target,
        };
        let from = install(&source, &all(&source.modules), &[]);
        let to = install(&target, &all(&target.modules), &[]);
        let set = diff(&from, &to);
        assert_eq!(
            (set.source, set.target),
            (OdooVersion::V18, OdooVersion::V19)
        );
        set.changes
    }

    fn name(value: &str) -> ModelName {
        ModelName::new(value)
    }

    #[test]
    fn models_and_modules_that_come_and_go() {
        let found = changes(
            vec![
                module(
                    "base",
                    &[],
                    vec![model("res.partner", Inherit::Absent, vec![], vec![])],
                ),
                module(
                    "legacy",
                    &["base"],
                    vec![model("legacy.thing", Inherit::Absent, vec![], vec![])],
                ),
            ],
            vec![module(
                "base",
                &[],
                vec![
                    model("res.partner", Inherit::Absent, vec![], vec![]),
                    model("res.partner.new", Inherit::Absent, vec![], vec![]),
                ],
            )],
        );
        assert_eq!(
            found,
            [
                Change::ModuleRemoved {
                    module: ModuleName::new("legacy"),
                    before: at("legacy/__manifest__.py", 1),
                },
                Change::ModelRemoved {
                    model: name("legacy.thing"),
                    had_table: Some(true),
                    before: at("models.py", 1),
                },
                Change::ModelAdded {
                    model: name("res.partner.new"),
                    after: at("models.py", 1),
                },
            ]
        );
    }

    #[test]
    fn fields_are_compared_across_every_extending_module() {
        let source = vec![
            module(
                "base",
                &[],
                vec![model(
                    "sale.order.line",
                    Inherit::Absent,
                    vec![
                        field("product_uom", FieldType::Many2one, Some("uom.uom"), 10),
                        field("tax_id", FieldType::Many2many, Some("account.tax"), 11),
                        field("note", FieldType::Char, None, 12),
                    ],
                    vec![],
                )],
            ),
            module(
                "sale_stock",
                &["base"],
                vec![model(
                    "sale.order.line",
                    extends("sale.order.line"),
                    vec![field(
                        "route_id",
                        FieldType::Many2one,
                        Some("stock.route"),
                        20,
                    )],
                    vec![],
                )],
            ),
        ];
        let target = vec![
            module(
                "base",
                &[],
                vec![model(
                    "sale.order.line",
                    Inherit::Absent,
                    vec![
                        field("product_uom_id", FieldType::Many2one, Some("uom.uom"), 30),
                        field(
                            "tax_id",
                            FieldType::Many2many,
                            Some("account.tax.group"),
                            31,
                        ),
                        field("note", FieldType::Html, None, 32),
                        // Odoo 19 moved it into the core definition.
                        field("route_id", FieldType::Many2one, Some("stock.route"), 33),
                    ],
                    vec![],
                )],
            ),
            module("sale_stock", &["base"], vec![]),
        ];
        let found = changes(source, target);
        assert_eq!(
            found,
            [
                Change::FieldTypeChanged {
                    model: name("sale.order.line"),
                    field: FieldName::new("note"),
                    from: FieldType::Char,
                    to: FieldType::Html,
                    before: at("models.py", 12),
                    after: at("models.py", 32),
                },
                Change::FieldRemoved {
                    model: name("sale.order.line"),
                    field: FieldName::new("product_uom"),
                    storage: Storage::Column,
                    before: at("models.py", 10),
                },
                Change::FieldAdded {
                    model: name("sale.order.line"),
                    field: FieldName::new("product_uom_id"),
                    storage: Storage::Column,
                    after: at("models.py", 30),
                },
                Change::FieldMoved {
                    model: name("sale.order.line"),
                    field: FieldName::new("route_id"),
                    from: ModuleName::new("sale_stock"),
                    to: ModuleName::new("base"),
                    before: at("models.py", 20),
                    after: at("models.py", 33),
                },
                Change::FieldComodelChanged {
                    model: name("sale.order.line"),
                    field: FieldName::new("tax_id"),
                    from: name("account.tax"),
                    to: name("account.tax.group"),
                    before: at("models.py", 11),
                    after: at("models.py", 31),
                },
            ]
        );
    }

    #[test]
    fn a_redefinition_without_comodel_keeps_the_earlier_one() {
        let base = |line| {
            module(
                "base",
                &[],
                vec![model(
                    "res.partner",
                    Inherit::Absent,
                    vec![field(
                        "parent_id",
                        FieldType::Many2one,
                        Some("res.partner"),
                        line,
                    )],
                    vec![],
                )],
            )
        };
        let redefine = module(
            "extra",
            &["base"],
            vec![model(
                "res.partner",
                extends("res.partner"),
                vec![field("parent_id", FieldType::Many2one, None, 50)],
                vec![],
            )],
        );
        let found = changes(vec![base(10)], vec![base(10), redefine]);
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn methods_report_removal_addition_signature_and_body() {
        let source = vec![
            module(
                "base",
                &[],
                vec![model(
                    "sale.order",
                    Inherit::Absent,
                    vec![],
                    vec![
                        method("action_confirm", &[], 1, 10),
                        method("_prepare_invoice", &[], 2, 11),
                        method("_old_helper", &[], 3, 12),
                    ],
                )],
            ),
            module(
                "sale_stock",
                &["base"],
                vec![model(
                    "sale.order",
                    extends("sale.order"),
                    vec![],
                    vec![method("action_confirm", &[], 4, 20)],
                )],
            ),
        ];
        let target = vec![
            module(
                "base",
                &[],
                vec![model(
                    "sale.order",
                    Inherit::Absent,
                    vec![],
                    vec![
                        method("action_confirm", &[], 1, 30),
                        method("_prepare_invoice", &["grouped"], 5, 31),
                        method("_new_helper", &[], 6, 32),
                    ],
                )],
            ),
            module(
                "sale_stock",
                &["base"],
                vec![model(
                    "sale.order",
                    extends("sale.order"),
                    vec![],
                    vec![method("action_confirm", &[], 7, 40)],
                )],
            ),
        ];
        let found = changes(source, target);
        assert_eq!(
            found,
            [
                Change::MethodAdded {
                    model: name("sale.order"),
                    method: "_new_helper".to_string(),
                    after: at("models.py", 32),
                },
                Change::MethodRemoved {
                    model: name("sale.order"),
                    method: "_old_helper".to_string(),
                    before: at("models.py", 12),
                },
                Change::MethodParamsChanged {
                    model: name("sale.order"),
                    method: "_prepare_invoice".to_string(),
                    from: vec![],
                    to: vec!["grouped".to_string()],
                    before: at("models.py", 11),
                    after: at("models.py", 31),
                },
                Change::MethodBodyChanged {
                    model: name("sale.order"),
                    method: "_prepare_invoice".to_string(),
                    module: ModuleName::new("base"),
                    before: at("models.py", 11),
                    after: at("models.py", 31),
                },
                Change::MethodBodyChanged {
                    model: name("sale.order"),
                    method: "action_confirm".to_string(),
                    module: ModuleName::new("sale_stock"),
                    before: at("models.py", 20),
                    after: at("models.py", 40),
                },
            ]
        );
    }

    #[test]
    fn a_renamed_model_carries_its_fields_and_comodels_along() {
        let packaging = |model_name: &str, fields: Vec<FieldDef>| {
            model(model_name, Inherit::Absent, fields, vec![])
        };
        let source = vec![module(
            "base",
            &[],
            vec![
                packaging(
                    "product.packaging",
                    vec![
                        field("name", FieldType::Char, None, 1),
                        field("qty", FieldType::Float, None, 2),
                        field("barcode", FieldType::Char, None, 3),
                        field(
                            "product_id",
                            FieldType::Many2one,
                            Some("product.product"),
                            4,
                        ),
                    ],
                ),
                packaging(
                    "product.product",
                    vec![field(
                        "packaging_ids",
                        FieldType::One2many,
                        Some("product.packaging"),
                        10,
                    )],
                ),
                packaging(
                    "sale.order.line",
                    vec![field(
                        "product_packaging_id",
                        FieldType::Many2one,
                        Some("product.packaging"),
                        20,
                    )],
                ),
            ],
        )];
        let target = vec![module(
            "base",
            &[],
            vec![
                packaging(
                    "product.uom",
                    vec![
                        field("name", FieldType::Char, None, 1),
                        field("qty", FieldType::Float, None, 2),
                        field("barcode", FieldType::Char, None, 3),
                        field("uom_id", FieldType::Many2one, Some("uom.uom"), 4),
                    ],
                ),
                packaging(
                    "product.product",
                    vec![field(
                        "product_uom_ids",
                        FieldType::One2many,
                        Some("product.uom"),
                        10,
                    )],
                ),
                packaging(
                    "sale.order.line",
                    vec![field(
                        "product_uom_line_id",
                        FieldType::Many2one,
                        Some("product.uom"),
                        20,
                    )],
                ),
            ],
        )];
        let all = |modules: &[Module]| -> Vec<ModuleName> {
            modules.iter().map(|m| m.name.clone()).collect()
        };
        let source = Addons {
            version: OdooVersion::V18,
            diagnostics: Vec::new(),
            modules: source,
        };
        let target = Addons {
            version: OdooVersion::V19,
            diagnostics: Vec::new(),
            modules: target,
        };
        let set = diff(
            &install(&source, &all(&source.modules), &[]),
            &install(&target, &all(&target.modules), &[]),
        );
        let model_renames: Vec<(&str, &str, Confidence, bool)> = set
            .renames
            .iter()
            .filter_map(|r| match r {
                Rename::Model {
                    from,
                    to,
                    confidence,
                    merged,
                    ..
                } => Some((from.as_str(), to.as_str(), *confidence, *merged)),
                _ => None,
            })
            .collect();
        assert_eq!(
            model_renames,
            [("product.packaging", "product.uom", Confidence::High, false)]
        );
        let field_renames: Vec<(&str, &str, &str)> = set
            .renames
            .iter()
            .filter_map(|r| match r {
                Rename::Field {
                    model, from, to, ..
                } => Some((model.as_str(), from.as_str(), to.as_str())),
                _ => None,
            })
            .collect();
        assert!(
            field_renames.contains(&("product.product", "packaging_ids", "product_uom_ids")),
            "{field_renames:?}"
        );
    }

    #[test]
    fn inherited_fields_count_and_delegated_ones_stay_with_their_model() {
        let mixin = |fields: Vec<FieldDef>| model("mail.thread", Inherit::Absent, fields, vec![]);
        let with_mixin = |name: &str, own: Vec<FieldDef>| ModelDef {
            name: Some(ModelName::new(name)),
            inherit: Inherit::List(vec![ModelName::new("mail.thread")]),
            ..model(name, Inherit::Absent, own, vec![])
        };
        let delegating = |fields: Vec<FieldDef>| ModelDef {
            inherits: vec![pontix_core::Delegation {
                parent: ModelName::new("res.partner"),
                field: FieldName::new("partner_id"),
            }],
            ..model("res.users", Inherit::Absent, fields, vec![])
        };
        let source = vec![module(
            "base",
            &[],
            vec![
                mixin(vec![field(
                    "message_ids",
                    FieldType::One2many,
                    Some("mail.message"),
                    1,
                )]),
                with_mixin("maintenance.request", vec![]),
                // `note` moves from the model into the mixin it inherits.
                with_mixin("crm.lead", vec![field("note", FieldType::Html, None, 5)]),
                model(
                    "res.partner",
                    Inherit::Absent,
                    vec![field("fax", FieldType::Char, None, 7)],
                    vec![],
                ),
                delegating(vec![]),
            ],
        )];
        let target = vec![module(
            "base",
            &[],
            vec![
                mixin(vec![
                    field("message_ids", FieldType::One2many, Some("mail.message"), 1),
                    field("note", FieldType::Html, None, 2),
                ]),
                // No longer inherits the mixin.
                model("maintenance.request", Inherit::Absent, vec![], vec![]),
                with_mixin("crm.lead", vec![]),
                model("res.partner", Inherit::Absent, vec![], vec![]),
                delegating(vec![]),
            ],
        )];
        let found = changes(source, target);
        assert_eq!(
            found,
            [
                Change::FieldAdded {
                    model: name("mail.thread"),
                    field: FieldName::new("note"),
                    storage: Storage::Column,
                    after: at("models.py", 2),
                },
                Change::ParentRemoved {
                    model: name("maintenance.request"),
                    parent: name("mail.thread"),
                    before: at("models.py", 1),
                },
                Change::FieldRemoved {
                    model: name("maintenance.request"),
                    field: FieldName::new("message_ids"),
                    // A One2many has no column of its own.
                    storage: Storage::NotStored,
                    before: at("models.py", 1),
                },
                // Reported once, on the model that owns it, not on `res.users`.
                Change::FieldRemoved {
                    model: name("res.partner"),
                    field: FieldName::new("fax"),
                    storage: Storage::Column,
                    before: at("models.py", 7),
                },
            ]
        );
    }

    #[test]
    fn storage_and_selection_keys_follow_every_declaration() {
        let with = |field: FieldDef, attrs: FieldAttrs| FieldDef { attrs, ..field };
        let keys = |values: &[&str]| Written::Value(values.iter().map(|v| v.to_string()).collect());
        let computed = || FieldAttrs {
            compute: Written::Value("_compute".to_string()),
            ..FieldAttrs::default()
        };
        let stored = || FieldAttrs {
            store: Written::Value(true),
            ..FieldAttrs::default()
        };
        let mixin = |fields: Vec<FieldDef>| ModelDef {
            base: ModelBase::Abstract,
            ..model("portal.mixin", Inherit::Absent, fields, vec![])
        };
        let source = vec![
            module(
                "base",
                &[],
                vec![
                    model(
                        "sale.order",
                        Inherit::Absent,
                        vec![
                            with(field("total", FieldType::Monetary, None, 2), computed()),
                            with(
                                field("state", FieldType::Selection, None, 3),
                                FieldAttrs {
                                    selection: keys(&["draft", "sent", "done"]),
                                    ..FieldAttrs::default()
                                },
                            ),
                        ],
                        vec![],
                    ),
                    mixin(vec![with(
                        field("access_url", FieldType::Char, None, 4),
                        computed(),
                    )]),
                ],
            ),
            // An extension stores the computed field.
            module(
                "sale",
                &["base"],
                vec![model(
                    "sale.order",
                    extends("sale.order"),
                    vec![with(field("total", FieldType::Monetary, None, 5), stored())],
                    vec![],
                )],
            ),
        ];
        let target = vec![
            module(
                "base",
                &[],
                vec![
                    model(
                        "sale.order",
                        Inherit::Absent,
                        vec![
                            with(field("total", FieldType::Monetary, None, 12), computed()),
                            with(
                                field("state", FieldType::Selection, None, 13),
                                FieldAttrs {
                                    selection: keys(&["draft", "done"]),
                                    ..FieldAttrs::default()
                                },
                            ),
                        ],
                        vec![],
                    ),
                    // Removed from an abstract model: it never had a column.
                    mixin(vec![]),
                ],
            ),
            module("sale", &["base"], vec![]),
        ];
        assert_eq!(
            changes(source, target),
            [
                Change::FieldRemoved {
                    model: name("portal.mixin"),
                    field: FieldName::new("access_url"),
                    storage: Storage::NotStored,
                    before: at("models.py", 4),
                },
                Change::SelectionKeysRemoved {
                    model: name("sale.order"),
                    field: FieldName::new("state"),
                    keys: vec!["sent".to_string()],
                    storage: Storage::Column,
                    before: at("models.py", 3),
                    after: at("models.py", 13),
                },
                Change::FieldStorageChanged {
                    model: name("sale.order"),
                    field: FieldName::new("total"),
                    from: Storage::Column,
                    to: Storage::NotStored,
                    before: at("models.py", 5),
                    after: at("models.py", 12),
                },
            ]
        );
    }

    #[test]
    fn a_redeclared_mixin_field_keeps_the_mixin_arguments() {
        let mixin = model(
            "mail.thread",
            Inherit::Absent,
            vec![FieldDef {
                attrs: FieldAttrs {
                    compute: Written::Value("_compute_count".to_string()),
                    ..FieldAttrs::default()
                },
                ..field("message_count", FieldType::Integer, None, 1)
            }],
            vec![],
        );
        let thread = |extra: Vec<FieldDef>| ModelDef {
            name: Some(name("crm.lead")),
            inherit: Inherit::List(vec![name("mail.thread")]),
            ..model("crm.lead", Inherit::Absent, extra, vec![])
        };
        // Only the label changes: still computed, still not stored.
        let source = vec![module(
            "base",
            &[],
            vec![
                mixin.clone(),
                thread(vec![field("message_count", FieldType::Integer, None, 8)]),
            ],
        )];
        let target = vec![module("base", &[], vec![mixin, thread(vec![])])];
        assert_eq!(changes(source, target), []);
    }

    #[test]
    fn a_field_moved_behind_delegation_leaves_the_table() {
        let source = vec![module(
            "base",
            &[],
            vec![model(
                "hr.employee",
                Inherit::Absent,
                vec![field("job_title", FieldType::Char, None, 3)],
                vec![],
            )],
        )];
        let employee = ModelDef {
            inherits: vec![pontix_core::Delegation {
                parent: name("hr.version"),
                field: FieldName::new("version_id"),
            }],
            ..model("hr.employee", Inherit::Absent, vec![], vec![])
        };
        let version = model(
            "hr.version",
            Inherit::Absent,
            vec![field("job_title", FieldType::Char, None, 9)],
            vec![],
        );
        let target = vec![module("base", &[], vec![employee, version])];
        assert_eq!(
            changes(source, target),
            [
                Change::FieldStorageChanged {
                    model: name("hr.employee"),
                    field: FieldName::new("job_title"),
                    from: Storage::Column,
                    to: Storage::NotStored,
                    before: at("models.py", 3),
                    after: at("models.py", 9),
                },
                Change::ModelAdded {
                    model: name("hr.version"),
                    after: at("models.py", 1),
                },
            ]
        );
    }
}
