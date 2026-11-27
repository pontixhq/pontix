//! One version's models as a whole: every field and method across all the
//! modules that define or extend each model, in load order, and those it
//! gets from the models it inherits from (`_inherit` of another model, such
//! as `mail.thread`). Fields reached through delegation (`_inherits`) are
//! kept apart: they are reachable, but they belong to the other model.

use std::collections::{BTreeMap, BTreeSet};

use crate::Installation;
use pontix_core::storage::{self, Storage};
use pontix_core::{
    FieldAttrs, FieldName, FieldType, Location, MethodDef, ModelName, ModuleName, Written,
};

/// A model as a whole in one version: everything its class definitions
/// declare, and what it reaches through other models.
pub struct ModelView<'a> {
    /// Where the model is first defined.
    pub first: Location,
    /// Whether it has a table: `None` when Pontix cannot tell.
    pub has_table: Option<bool>,
    /// Fields its own definitions declare.
    pub fields: BTreeMap<&'a FieldName, FieldView<'a>>,
    /// Methods its own definitions define.
    pub methods: BTreeMap<&'a str, MethodView<'a>>,
    /// Fields it only has through another model, with where they are
    /// declared. A model that stops inheriting `mail.thread` loses
    /// `message_ids` although none of its own code changed.
    pub inherited_fields: BTreeMap<&'a FieldName, FieldView<'a>>,
    /// Methods it only has through another model, with where they are
    /// defined.
    pub inherited_methods: BTreeMap<&'a str, &'a Location>,
    /// Fields reachable through `_inherits`, as `email` on `res.users`
    /// through `res.partner`. Their changes are reported on that model.
    pub delegated_fields: BTreeMap<&'a FieldName, FieldView<'a>>,
    /// The other models it inherits from.
    pub parents: BTreeSet<ModelName>,
    /// The models it delegates to.
    pub delegates: BTreeSet<ModelName>,
}

impl<'a> ModelView<'a> {
    /// Whether the field can be reached on the model at all: own,
    /// inherited, or through delegation.
    pub fn reaches_field(&self, field: &FieldName) -> bool {
        self.field_location(field).is_some() || self.delegated_fields.contains_key(field)
    }

    /// A field the model has, own or inherited.
    pub fn field(&self, field: &FieldName) -> Option<&FieldView<'a>> {
        self.fields
            .get(field)
            .or_else(|| self.inherited_fields.get(field))
    }

    /// Where a field the model has, own or inherited, is declared.
    pub fn field_location(&self, field: &FieldName) -> Option<&'a Location> {
        self.field(field).map(|view| view.first)
    }

    /// Where a field the model has keeps its values on this model, whose
    /// table decides as much as the field's own arguments.
    pub fn storage(&self, field: &FieldView<'_>) -> Storage {
        storage::storage(field.field_type, &field.attrs, self.has_table)
    }

    /// Whether the model has the method, own or inherited.
    pub fn has_method(&self, method: &str) -> bool {
        self.methods.contains_key(method) || self.inherited_methods.contains_key(method)
    }
}

/// A field as a whole: its first declaration, and what the declarations in
/// load order make of its type, comodel and arguments.
#[derive(Clone)]
pub struct FieldView<'a> {
    /// The module whose definition declares the field first.
    pub owner: &'a ModuleName,
    /// The first declaration.
    pub first: &'a Location,
    /// The type of the last declaration, which is the one Odoo keeps.
    pub field_type: &'a FieldType,
    /// Where that last declaration is.
    pub type_at: &'a Location,
    /// The comodel of the last declaration that names one, and where.
    pub comodel: Option<(&'a ModelName, &'a Location)>,
    /// The arguments every declaration writes, merged in load order.
    pub attrs: FieldAttrs,
}

impl FieldView<'_> {
    /// The keys a selection field accepts.
    pub fn selection_keys(&self) -> Written<Vec<String>> {
        storage::selection_keys(&self.attrs)
    }

    /// Adds a later declaration: one of another type replaces the field and
    /// its arguments.
    fn redeclare(&mut self, field_type: &FieldType, attrs: &FieldAttrs) {
        self.attrs = if self.field_type == field_type {
            storage::merge(&self.attrs, attrs)
        } else {
            attrs.clone()
        };
    }
}

/// A method as a whole: its first definition and every module's own.
pub struct MethodView<'a> {
    /// The first definition in load order: the method's own signature.
    pub first: &'a MethodDef,
    /// Each module's definition; a module that defines it twice keeps the
    /// later one, as Python does.
    pub by_module: BTreeMap<&'a ModuleName, &'a MethodDef>,
}

/// Every model of the installation, built from its class definitions in
/// load order.
pub fn models<'a>(installation: &Installation<'a>) -> BTreeMap<ModelName, ModelView<'a>> {
    let mut views = BTreeMap::new();
    for (name, definitions) in installation.models() {
        let Some(first) = definitions.first() else {
            continue;
        };
        // The class that defines the model decides its kind; any class may
        // set `_auto` or `_table_query`, and the last one to set `_auto` wins.
        let auto = definitions
            .iter()
            .rev()
            .map(|definition| &definition.def.auto)
            .find(|auto| auto.is_given())
            .unwrap_or(&Written::Absent);
        let table_query = definitions
            .iter()
            .any(|definition| definition.def.table_query);
        let mut view = ModelView {
            first: first.def.location.clone(),
            has_table: storage::has_table(first.def.base, auto, table_query),
            fields: BTreeMap::new(),
            methods: BTreeMap::new(),
            inherited_fields: BTreeMap::new(),
            inherited_methods: BTreeMap::new(),
            delegated_fields: BTreeMap::new(),
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
                    attrs: FieldAttrs::default(),
                });
                entry.redeclare(&field.field_type, &field.attrs);
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
fn delegate<'a>(views: &mut BTreeMap<ModelName, ModelView<'a>>) {
    let names: Vec<ModelName> = views.keys().cloned().collect();
    for name in names {
        let mut reached = BTreeMap::new();
        let mut seen = BTreeSet::from([name.clone()]);
        let mut pending: Vec<ModelName> = views[&name].delegates.iter().cloned().collect();
        while let Some(target) = pending.pop() {
            if !seen.insert(target.clone()) {
                continue;
            }
            let Some(view) = views.get(&target) else {
                continue;
            };
            for (field, found) in view.fields.iter().chain(&view.inherited_fields) {
                reached.entry(*field).or_insert_with(|| found.clone());
            }
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
                fields.entry(*field).or_insert_with(|| own.clone());
            }
            for (method, own) in &view.methods {
                methods.entry(*method).or_insert(&own.first.location);
            }
            pending.extend(view.parents.iter().cloned());
        }
        let Some(view) = views.get_mut(&name) else {
            continue;
        };
        // A model's own declaration of a field it also inherits builds on
        // the inherited one, as Odoo merges the arguments of every class.
        for (field, own) in &mut view.fields {
            if let Some(inherited) = fields.get(field)
                && inherited.field_type == own.field_type
            {
                own.attrs = storage::merge(&inherited.attrs, &own.attrs);
            }
        }
        fields.retain(|field, _| !view.fields.contains_key(field));
        methods.retain(|method, _| !view.methods.contains_key(method));
        view.inherited_fields = fields;
        view.inherited_methods = methods;
    }
}
