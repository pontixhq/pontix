//! One version's models as a whole: every field and method across all the
//! modules that define or extend each model, in load order, and those it
//! gets from the models it inherits from (`_inherit` of another model, such
//! as `mail.thread`). Fields reached through delegation (`_inherits`) are
//! kept apart: they are reachable, but they belong to the other model.

use std::collections::{BTreeMap, BTreeSet};

use pontix_core::{FieldName, FieldType, Location, MethodDef, ModelName, ModuleName};
use pontix_index::Installation;

pub(crate) struct ModelView<'a> {
    /// Where the model is first defined.
    pub(crate) first: Location,
    /// Fields its own definitions declare.
    pub(crate) fields: BTreeMap<&'a FieldName, FieldView<'a>>,
    /// Methods its own definitions define.
    pub(crate) methods: BTreeMap<&'a str, MethodView<'a>>,
    /// Fields it only has through another model, with where they are
    /// declared. A model that stops inheriting `mail.thread` loses
    /// `message_ids` although none of its own code changed.
    pub(crate) inherited_fields: BTreeMap<&'a FieldName, &'a Location>,
    /// Methods it only has through another model, with where they are
    /// defined.
    pub(crate) inherited_methods: BTreeMap<&'a str, &'a Location>,
    /// Fields reachable through `_inherits`, as `email` on `res.users`
    /// through `res.partner`. Their changes are reported on that model.
    pub(crate) delegated_fields: BTreeSet<&'a FieldName>,
    /// The other models it inherits from.
    pub(crate) parents: BTreeSet<ModelName>,
    /// The models it delegates to.
    delegates: BTreeSet<ModelName>,
}

impl<'a> ModelView<'a> {
    /// Whether the field can be reached on the model at all: own,
    /// inherited, or through delegation.
    pub(crate) fn reaches_field(&self, field: &FieldName) -> bool {
        self.field_location(field).is_some() || self.delegated_fields.contains(field)
    }

    /// Where a field the model has, own or inherited, is declared.
    pub(crate) fn field_location(&self, field: &FieldName) -> Option<&'a Location> {
        self.fields
            .get(field)
            .map(|view| view.first)
            .or_else(|| self.inherited_fields.get(field).copied())
    }

    /// Whether the model has the method, own or inherited.
    pub(crate) fn has_method(&self, method: &str) -> bool {
        self.methods.contains_key(method) || self.inherited_methods.contains_key(method)
    }
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
            inherited_fields: BTreeMap::new(),
            inherited_methods: BTreeMap::new(),
            delegated_fields: BTreeSet::new(),
            parents: BTreeSet::new(),
            delegates: BTreeSet::new(),
        };
        for definition in &definitions {
            view.parents.extend(
                definition
                    .def
                    .parents()
                    .iter()
                    .filter(|parent| **parent != name)
                    .cloned(),
            );
            view.delegates
                .extend(definition.def.inherits.iter().map(|d| d.parent.clone()));
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
    inherit(&mut views);
    delegate(&mut views);
    views
}

/// Gives each model the fields of the models it delegates to, own and
/// inherited, through any depth.
fn delegate(views: &mut BTreeMap<ModelName, ModelView<'_>>) {
    let names: Vec<ModelName> = views.keys().cloned().collect();
    for name in names {
        let mut reached = BTreeSet::new();
        let mut seen = BTreeSet::from([name.clone()]);
        let mut pending: Vec<ModelName> = views[&name].delegates.iter().cloned().collect();
        while let Some(target) = pending.pop() {
            if !seen.insert(target.clone()) {
                continue;
            }
            let Some(view) = views.get(&target) else {
                continue;
            };
            reached.extend(view.fields.keys().copied());
            reached.extend(view.inherited_fields.keys().copied());
            pending.extend(view.delegates.iter().cloned());
        }
        if let Some(view) = views.get_mut(&name) {
            view.delegated_fields = reached;
        }
    }
}

/// Gives each model the fields and methods of the models it inherits from,
/// through any depth, keeping its own where both have one.
fn inherit<'a>(views: &mut BTreeMap<ModelName, ModelView<'a>>) {
    let names: Vec<ModelName> = views.keys().cloned().collect();
    for name in names {
        let mut fields = BTreeMap::new();
        let mut methods = BTreeMap::new();
        let mut seen = BTreeSet::from([name.clone()]);
        let mut pending: Vec<ModelName> = views[&name].parents.iter().cloned().collect();
        while let Some(parent) = pending.pop() {
            if !seen.insert(parent.clone()) {
                continue;
            }
            let Some(view) = views.get(&parent) else {
                continue;
            };
            for (field, own) in &view.fields {
                fields.entry(*field).or_insert(own.first);
            }
            for (method, own) in &view.methods {
                methods.entry(*method).or_insert(&own.first.location);
            }
            pending.extend(view.parents.iter().cloned());
        }
        let Some(view) = views.get_mut(&name) else {
            continue;
        };
        fields.retain(|field, _| !view.fields.contains_key(field));
        methods.retain(|method, _| !view.methods.contains_key(method));
        view.inherited_fields = fields;
        view.inherited_methods = methods;
    }
}
