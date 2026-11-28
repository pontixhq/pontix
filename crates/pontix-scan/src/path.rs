//! Field paths such as `order_id.partner_id.country_id`, followed model by
//! model through each relational field's comodel.

use std::collections::{BTreeMap, BTreeSet};

use pontix_core::{FieldName, ModelName, ModuleName};
use pontix_index::registry::{FieldView, ModelView};

/// Fields every model has without declaring them, with their comodel
/// (`BaseModel` in `odoo/orm/models.py`).
const MAGIC_FIELDS: [(&str, Option<&str>); 6] = [
    ("id", None),
    ("display_name", None),
    ("create_date", None),
    ("write_date", None),
    ("create_uid", Some("res.users")),
    ("write_uid", Some("res.users")),
];

/// The modules a check may rely on: those the checked module installs,
/// its dependencies and the `auto_install` modules they bring. A name that
/// only other modules declare is not there for it, even when an installation
/// holding every module being upgraded has it.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Scope<'s> {
    /// Every module of the installation.
    All,
    /// Only these.
    Modules(&'s BTreeSet<ModuleName>),
}

impl Scope<'_> {
    /// Whether the module is in scope.
    pub(crate) fn has(self, module: &ModuleName) -> bool {
        match self {
            Self::All => true,
            Self::Modules(modules) => modules.contains(module),
        }
    }

    /// Whether a module in scope declares the field.
    pub(crate) fn has_field(self, field: &FieldView<'_>) -> bool {
        field.modules.iter().any(|module| self.has(module))
    }
}

/// How far a path gets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Walk {
    /// Every field it names exists, as far as Pontix can follow it: a field
    /// whose comodel it does not know ends the walk without a verdict.
    Reaches,
    /// A field it names does not exist on the model reached so far.
    Missing {
        /// The model the field was looked up on.
        model: ModelName,
        /// The missing field.
        field: FieldName,
    },
}

/// Follows `path` from `start`.
pub(crate) fn walk(
    models: &BTreeMap<ModelName, ModelView<'_>>,
    scope: Scope<'_>,
    start: &ModelName,
    path: &str,
) -> Walk {
    let mut model = start.clone();
    let mut segments = path
        .split('.')
        .filter(|segment| !segment.is_empty())
        .peekable();
    while let Some(segment) = segments.next() {
        // A model that does not exist is reported where it is used.
        let Some(view) = models.get(&model) else {
            return Walk::Reaches;
        };
        let field = FieldName::new(segment);
        let comodel = if let Some(found) = lookup(view, &field, scope) {
            found.comodel.map(|(comodel, _)| comodel.clone())
        } else if let Some((_, comodel)) = MAGIC_FIELDS.iter().find(|(name, _)| *name == segment) {
            comodel.map(ModelName::new)
        } else {
            return Walk::Missing { model, field };
        };
        if segments.peek().is_none() {
            break;
        }
        match comodel {
            Some(next) => model = next,
            None => break,
        }
    }
    Walk::Reaches
}

/// The model whose records `path` gives from `start`, when every field it
/// names exists and is relational with a known comodel. An empty path gives
/// `start` itself.
pub(crate) fn reach(
    models: &BTreeMap<ModelName, ModelView<'_>>,
    scope: Scope<'_>,
    start: &ModelName,
    path: &[String],
) -> Option<ModelName> {
    let mut model = start.clone();
    for segment in path {
        let view = models.get(&model)?;
        let field = FieldName::new(segment.as_str());
        let comodel = if let Some(found) = lookup(view, &field, scope) {
            found.comodel.map(|(comodel, _)| comodel.clone())
        } else {
            MAGIC_FIELDS
                .iter()
                .find(|(name, _)| *name == segment)
                .and_then(|(_, comodel)| comodel.map(ModelName::new))
        };
        model = comodel?;
    }
    models.contains_key(&model).then_some(model)
}

/// A field the model has, own, inherited, or through delegation, that a
/// module in scope declares.
pub(crate) fn lookup<'v, 'a>(
    view: &'v ModelView<'a>,
    field: &FieldName,
    scope: Scope<'_>,
) -> Option<&'v FieldView<'a>> {
    // A model's own declaration may come from a module out of scope while
    // the field it inherits does not.
    [&view.fields, &view.inherited_fields, &view.delegated_fields]
        .into_iter()
        .filter_map(|fields| fields.get(field))
        .find(|found| scope.has_field(found))
}
