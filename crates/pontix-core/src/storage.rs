//! Where a field's values live in the database, by Odoo's rules: which
//! arguments a redefinition keeps, which fields are stored, and which models
//! have a table (`odoo/orm/fields.py`, `odoo/orm/models.py`, Odoo 19; the
//! same rules hold in 18).

use crate::field_type::FieldType;
use crate::symbol::{FieldAttrs, ModelBase, Written};

/// Where a field's values live.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Storage {
    /// A column of the model's table.
    Column,
    /// A relation table, for a stored `Many2many`.
    RelationTable,
    /// `ir.attachment` records, for a `Binary` or `Image` field with
    /// `attachment=True`, the default.
    Attachment,
    /// Nowhere: computed or related without `store=True`, a `One2many`, or a
    /// field of a model without a table.
    NotStored,
    /// Pontix cannot tell, because an argument or the model's table is not
    /// written as a literal.
    Unknown,
}

impl Storage {
    /// Whether the database holds the field's values: `None` when unknown.
    pub fn holds_data(self) -> Option<bool> {
        match self {
            Self::Column | Self::RelationTable | Self::Attachment => Some(true),
            Self::NotStored => Some(false),
            Self::Unknown => None,
        }
    }
}

/// The arguments of a field after a later declaration of the same type.
/// The later one overrides each argument it writes. `selection=` starts the
/// keys over, and every `selection_add=` after it adds to them
/// (`Field._setup_attrs`, `Selection._setup_attrs`). A declaration of another
/// type replaces the field, so the caller starts over from it instead.
pub fn merge(earlier: &FieldAttrs, later: &FieldAttrs) -> FieldAttrs {
    let (selection, selection_add) = if later.selection.is_given() {
        (later.selection.clone(), later.selection_add.clone())
    } else {
        (
            earlier.selection.clone(),
            add_keys(&earlier.selection_add, &later.selection_add),
        )
    };
    FieldAttrs {
        store: pick(&earlier.store, &later.store),
        compute: pick(&earlier.compute, &later.compute),
        related: pick(&earlier.related, &later.related),
        attachment: pick(&earlier.attachment, &later.attachment),
        selection,
        selection_add,
    }
}

/// The later value when it is given, the earlier one otherwise.
fn pick<T: Clone>(earlier: &Written<T>, later: &Written<T>) -> Written<T> {
    if later.is_given() {
        later.clone()
    } else {
        earlier.clone()
    }
}

fn add_keys(earlier: &Written<Vec<String>>, later: &Written<Vec<String>>) -> Written<Vec<String>> {
    match (earlier, later) {
        (Written::Unreadable, _) | (_, Written::Unreadable) => Written::Unreadable,
        (Written::Absent, other) | (other, Written::Absent) => other.clone(),
        (Written::Value(first), Written::Value(then)) => {
            Written::Value(first.iter().chain(then).cloned().collect())
        }
    }
}

/// The keys a selection field accepts: `selection=` and every
/// `selection_add=`, without repeats.
pub fn selection_keys(attrs: &FieldAttrs) -> Written<Vec<String>> {
    match add_keys(&attrs.selection, &attrs.selection_add) {
        Written::Value(keys) => {
            let mut unique = Vec::with_capacity(keys.len());
            for key in keys {
                if !unique.contains(&key) {
                    unique.push(key);
                }
            }
            Written::Value(unique)
        }
        other => other,
    }
}

/// Whether a model has a table: `None` when Pontix cannot tell. An abstract
/// model has none, nor does one with `_auto = False` or a `_table_query`.
pub fn has_table(base: ModelBase, auto: &Written<bool>, table_query: bool) -> Option<bool> {
    match base {
        ModelBase::Abstract => return Some(false),
        ModelBase::Unknown => return None,
        ModelBase::Model | ModelBase::Transient => {}
    }
    if table_query {
        return Some(false);
    }
    match auto {
        Written::Absent => Some(true),
        Written::Value(auto) => Some(*auto),
        Written::Unreadable => None,
    }
}

/// Where a field with these (merged) arguments keeps its values, on a model
/// with or without a table. Computed and related fields are not stored
/// unless `store=True` says so.
pub fn storage(field_type: &FieldType, attrs: &FieldAttrs, has_table: Option<bool>) -> Storage {
    match has_table {
        Some(false) => return Storage::NotStored,
        None => return Storage::Unknown,
        Some(true) => {}
    }
    if *field_type == FieldType::One2many {
        return Storage::NotStored;
    }
    let stored = match attrs.store {
        Written::Value(store) => store,
        Written::Unreadable => return Storage::Unknown,
        Written::Absent => !attrs.compute.is_given() && !attrs.related.is_given(),
    };
    if !stored {
        return Storage::NotStored;
    }
    match field_type {
        FieldType::Many2many => Storage::RelationTable,
        FieldType::Binary | FieldType::Image => match attrs.attachment {
            Written::Absent | Written::Value(true) => Storage::Attachment,
            Written::Value(false) => Storage::Column,
            Written::Unreadable => Storage::Unknown,
        },
        _ => Storage::Column,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(values: &[&str]) -> Written<Vec<String>> {
        Written::Value(values.iter().map(|key| key.to_string()).collect())
    }

    #[test]
    fn computed_and_related_fields_are_stored_only_when_asked() {
        let stored = |attrs: FieldAttrs| storage(&FieldType::Char, &attrs, Some(true));
        assert_eq!(stored(FieldAttrs::default()), Storage::Column);
        let computed = FieldAttrs {
            compute: Written::Value("_compute_name".into()),
            ..FieldAttrs::default()
        };
        assert_eq!(stored(computed.clone()), Storage::NotStored);
        let stored_computed = FieldAttrs {
            store: Written::Value(true),
            ..computed
        };
        assert_eq!(stored(stored_computed), Storage::Column);
        let related = FieldAttrs {
            related: Written::Value("partner_id.name".into()),
            ..FieldAttrs::default()
        };
        assert_eq!(stored(related), Storage::NotStored);
        let unreadable = FieldAttrs {
            store: Written::Unreadable,
            ..FieldAttrs::default()
        };
        assert_eq!(stored(unreadable), Storage::Unknown);
    }

    #[test]
    fn type_and_table_decide_the_rest() {
        let plain = FieldAttrs::default();
        assert_eq!(
            storage(&FieldType::One2many, &plain, Some(true)),
            Storage::NotStored
        );
        assert_eq!(
            storage(&FieldType::Many2many, &plain, Some(true)),
            Storage::RelationTable
        );
        assert_eq!(
            storage(&FieldType::Image, &plain, Some(true)),
            Storage::Attachment
        );
        let in_column = FieldAttrs {
            attachment: Written::Value(false),
            ..FieldAttrs::default()
        };
        assert_eq!(
            storage(&FieldType::Binary, &in_column, Some(true)),
            Storage::Column
        );
        assert_eq!(
            storage(&FieldType::Char, &plain, Some(false)),
            Storage::NotStored
        );
        assert_eq!(storage(&FieldType::Char, &plain, None), Storage::Unknown);
    }

    #[test]
    fn tables_follow_the_base_auto_and_table_query() {
        assert_eq!(
            has_table(ModelBase::Model, &Written::Absent, false),
            Some(true)
        );
        assert_eq!(
            has_table(ModelBase::Transient, &Written::Absent, false),
            Some(true)
        );
        assert_eq!(
            has_table(ModelBase::Abstract, &Written::Absent, false),
            Some(false)
        );
        assert_eq!(
            has_table(ModelBase::Model, &Written::Value(false), false),
            Some(false)
        );
        assert_eq!(
            has_table(ModelBase::Model, &Written::Absent, true),
            Some(false)
        );
        assert_eq!(
            has_table(ModelBase::Model, &Written::Unreadable, false),
            None
        );
        assert_eq!(has_table(ModelBase::Unknown, &Written::Absent, false), None);
    }

    #[test]
    fn a_redefinition_overrides_only_what_it_writes() {
        let original = FieldAttrs {
            compute: Written::Value("_compute_total".into()),
            ..FieldAttrs::default()
        };
        let extension = FieldAttrs {
            store: Written::Value(true),
            ..FieldAttrs::default()
        };
        let merged = merge(&original, &extension);
        assert_eq!(merged.compute, Written::Value("_compute_total".into()));
        assert_eq!(merged.store, Written::Value(true));
        assert_eq!(
            storage(&FieldType::Monetary, &merged, Some(true)),
            Storage::Column
        );
    }

    #[test]
    fn selection_keys_add_up_until_a_new_selection() {
        let original = FieldAttrs {
            selection: keys(&["draft", "done"]),
            ..FieldAttrs::default()
        };
        let added = FieldAttrs {
            selection_add: keys(&["sent", "done"]),
            ..FieldAttrs::default()
        };
        let merged = merge(&original, &added);
        assert_eq!(selection_keys(&merged), keys(&["draft", "done", "sent"]));
        let replaced = FieldAttrs {
            selection: keys(&["new"]),
            ..FieldAttrs::default()
        };
        assert_eq!(selection_keys(&merge(&merged, &replaced)), keys(&["new"]));
        let dynamic = FieldAttrs {
            selection: Written::Unreadable,
            ..FieldAttrs::default()
        };
        assert_eq!(
            selection_keys(&merge(&dynamic, &added)),
            Written::Unreadable
        );
    }
}
