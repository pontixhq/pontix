//! Field paths such as `order_id.partner_id.country_id`, followed model by
//! model through each relational field's comodel.

use std::collections::BTreeMap;

use pontix_core::{FieldName, ModelName};
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
        let comodel = if let Some(found) = lookup(view, &field) {
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
    start: &ModelName,
    path: &[String],
) -> Option<ModelName> {
    let mut model = start.clone();
    for segment in path {
        let view = models.get(&model)?;
        let field = FieldName::new(segment.as_str());
        let comodel = if let Some(found) = lookup(view, &field) {
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

/// A field the model has: own, inherited, or through delegation.
pub(crate) fn lookup<'v, 'a>(
    view: &'v ModelView<'a>,
    field: &FieldName,
) -> Option<&'v FieldView<'a>> {
    view.field(field)
        .or_else(|| view.delegated_fields.get(field))
}
