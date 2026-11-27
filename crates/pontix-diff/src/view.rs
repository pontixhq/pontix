//! One version's models as a whole: every field and method across all the
//! modules that define or extend each model, in load order.

use std::collections::BTreeMap;

use pontix_core::{FieldName, FieldType, Location, MethodDef, ModelName, ModuleName};
use pontix_index::Installation;

pub(crate) struct ModelView<'a> {
    /// Where the model is first defined.
    pub(crate) first: Location,
    pub(crate) fields: BTreeMap<&'a FieldName, FieldView<'a>>,
    pub(crate) methods: BTreeMap<&'a str, MethodView<'a>>,
}

pub(crate) struct FieldView<'a> {
    /// The module whose definition declares the field first.
    pub(crate) owner: &'a ModuleName,
    /// The first declaration.
    pub(crate) first: &'a Location,
    /// The type of the last declaration, which is the one Odoo keeps.
    pub(crate) field_type: &'a FieldType,
    /// Where that last declaration is.
    pub(crate) type_at: &'a Location,
    /// The comodel of the last declaration that names one, and where.
    pub(crate) comodel: Option<(&'a ModelName, &'a Location)>,
}

pub(crate) struct MethodView<'a> {
    /// The first definition in load order: the method's own signature.
    pub(crate) first: &'a MethodDef,
    /// Each module's definition; a module that defines it twice keeps the
    /// later one, as Python does.
    pub(crate) by_module: BTreeMap<&'a ModuleName, &'a MethodDef>,
}

/// Every model of the installation, built from its class definitions in
/// load order.
pub(crate) fn models<'a>(installation: &Installation<'a>) -> BTreeMap<ModelName, ModelView<'a>> {
    let mut views = BTreeMap::new();
    for (name, definitions) in installation.models() {
        let Some(first) = definitions.first() else {
            continue;
        };
        let mut view = ModelView {
            first: first.def.location.clone(),
            fields: BTreeMap::new(),
            methods: BTreeMap::new(),
        };
        for definition in &definitions {
            for field in &definition.def.fields {
                let entry = view.fields.entry(&field.name).or_insert(FieldView {
                    owner: &definition.module.name,
                    first: &field.location,
                    field_type: &field.field_type,
                    type_at: &field.location,
                    comodel: None,
                });
                entry.field_type = &field.field_type;
                entry.type_at = &field.location;
                if let Some(comodel) = &field.comodel {
                    entry.comodel = Some((comodel, &field.location));
                }
            }
            for method in &definition.def.methods {
                let entry = view
                    .methods
                    .entry(method.name.as_str())
                    .or_insert(MethodView {
                        first: method,
                        by_module: BTreeMap::new(),
                    });
                entry.by_module.insert(&definition.module.name, method);
            }
        }
        views.insert(name, view);
    }
    views
}
