//! What changed between two Odoo versions: the `ChangeSet`.
//!
//! Each change carries where it can be seen: the definition in the source
//! version, the one in the target version, or both. A removal is shown by
//! its source definition alone.

use crate::field_type::FieldType;
use crate::manifest::ModuleName;
use crate::storage::Storage;
use crate::symbol::{FieldName, Location, ModelName};
use crate::version::OdooVersion;

/// Every change found between a source and a target Odoo version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSet {
    /// The version upgraded from.
    pub source: OdooVersion,
    /// The version upgraded to.
    pub target: OdooVersion,
    /// The changes, ordered by model, then by what they are about.
    pub changes: Vec<Change>,
    /// Removals and additions that look like one thing renamed. Never
    /// certain: each comes with how confident Pontix is and why.
    pub renames: Vec<Rename>,
}

/// How strongly the evidence points to a rename.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Confidence {
    /// Plausible, but the evidence is thin: check before relying on it.
    Low,
    /// Likely.
    Medium,
    /// The evidence agrees from several sides.
    High,
}

/// One reason to believe a rename.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Evidence {
    /// The names are the same once `_id`, `_ids` and plural endings are
    /// dropped, as in `tax_id` and `tax_ids`.
    SameStem,
    /// The names are this similar, from 0 to 100.
    SimilarName(u8),
    /// One name, without `_id`/`_ids`, ends or starts with the other, as
    /// `stock_move_ids` and `move_ids`, or `date_start` and
    /// `contract_date_start`.
    NameContained,
    /// The two fields have the same type, and the same comodel when they
    /// are relational.
    SameShape,
    /// They are the only removed and the only added field of that shape on
    /// the model.
    OnlyCandidate,
    /// This many fields elsewhere now point to the new model instead of the
    /// old one: same field, new comodel.
    FieldsRepointed(u32),
    /// On this many models, a field pointing to the old model was removed
    /// while a field pointing to the new one was added.
    ReferencesSwapped(u32),
    /// The old and new model share this share of their field names, from 0
    /// to 100, counting only fields besides the ones every model has.
    SharedFields(u8),
    /// This many fields moved from the old module to the new one.
    FieldsMoved(u32),
}

/// Something that looks renamed between the two versions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rename {
    /// A removed module whose fields moved to another module.
    Module {
        /// The removed module.
        from: ModuleName,
        /// The module that took its fields over.
        to: ModuleName,
        /// Whether `to` already existed in the source version, so the old
        /// module was merged into it rather than renamed.
        merged: bool,
        /// How likely.
        confidence: Confidence,
        /// Why.
        evidence: Vec<Evidence>,
    },
    /// A removed model that looks continued by another one.
    Model {
        /// The removed model.
        from: ModelName,
        /// The model that continues it.
        to: ModelName,
        /// Whether `to` already existed in the source version, so the old
        /// model was merged into it rather than renamed.
        merged: bool,
        /// How likely.
        confidence: Confidence,
        /// Why.
        evidence: Vec<Evidence>,
    },
    /// A removed field that looks continued by an added one, on the same
    /// model or on the model it was renamed to.
    Field {
        /// The model in the source version.
        model: ModelName,
        /// The removed field.
        from: FieldName,
        /// The model in the target version, when it was renamed too.
        to_model: ModelName,
        /// The added field.
        to: FieldName,
        /// How likely.
        confidence: Confidence,
        /// Why.
        evidence: Vec<Evidence>,
    },
}

/// One difference between the source and the target version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// An installable module is gone, or no longer installable.
    ModuleRemoved {
        /// The module.
        module: ModuleName,
        /// Its manifest in the source version.
        before: Location,
    },
    /// A model no module defines any more.
    ModelRemoved {
        /// The model.
        model: ModelName,
        /// Whether it had a table in the source version: `None` when Pontix
        /// cannot tell. A table means rows the upgrade can lose.
        had_table: Option<bool>,
        /// Its first definition in the source version.
        before: Location,
    },
    /// A model the source version did not have.
    ModelAdded {
        /// The model.
        model: ModelName,
        /// Its first definition in the target version.
        after: Location,
    },
    /// The model inherits from another model it did not inherit from, such
    /// as `mail.thread`, and so gains its fields and methods.
    ParentAdded {
        /// The model.
        model: ModelName,
        /// The model it now inherits from.
        parent: ModelName,
        /// The model's first definition in the target version.
        after: Location,
    },
    /// The model no longer inherits from a model, and loses the fields and
    /// methods it had only from there (`message_post` when it drops
    /// `mail.thread`).
    ParentRemoved {
        /// The model.
        model: ModelName,
        /// The model it no longer inherits from.
        parent: ModelName,
        /// The model's first definition in the source version.
        before: Location,
    },
    /// A field no definition of its model declares any more.
    FieldRemoved {
        /// The model.
        model: ModelName,
        /// The field.
        field: FieldName,
        /// Where its values lived in the source version. A stored field
        /// that goes away takes its data with it.
        storage: Storage,
        /// Its first declaration in the source version.
        before: Location,
    },
    /// A field the model did not have in the source version.
    FieldAdded {
        /// The model.
        model: ModelName,
        /// The field.
        field: FieldName,
        /// Where its values live in the target version.
        storage: Storage,
        /// Its first declaration in the target version.
        after: Location,
    },
    /// The field keeps its values somewhere else, for example a computed
    /// field that is no longer stored, whose column the upgrade drops.
    FieldStorageChanged {
        /// The model.
        model: ModelName,
        /// The field.
        field: FieldName,
        /// Where its values lived in the source version.
        from: Storage,
        /// Where they live in the target version.
        to: Storage,
        /// Its last declaration in the source version.
        before: Location,
        /// Its last declaration in the target version.
        after: Location,
    },
    /// A selection field no longer accepts some keys, which rows may still
    /// hold.
    SelectionKeysRemoved {
        /// The model.
        model: ModelName,
        /// The field.
        field: FieldName,
        /// The keys it no longer accepts, in source order.
        keys: Vec<String>,
        /// Where its values lived in the source version: rows hold the old
        /// keys only when it was stored.
        storage: Storage,
        /// Its last declaration in the source version.
        before: Location,
        /// Its last declaration in the target version.
        after: Location,
    },
    /// The field's type changed, for example `Char` to `Many2one`.
    FieldTypeChanged {
        /// The model.
        model: ModelName,
        /// The field.
        field: FieldName,
        /// Type in the source version.
        from: FieldType,
        /// Type in the target version.
        to: FieldType,
        /// Where its values lived in the source version: stored values must
        /// be converted.
        storage: Storage,
        /// Where the source type is declared.
        before: Location,
        /// Where the target type is declared.
        after: Location,
    },
    /// The relational field points to another model.
    FieldComodelChanged {
        /// The model.
        model: ModelName,
        /// The field.
        field: FieldName,
        /// Comodel in the source version.
        from: ModelName,
        /// Comodel in the target version.
        to: ModelName,
        /// Where the source comodel is declared.
        before: Location,
        /// Where the target comodel is declared.
        after: Location,
    },
    /// The field is now introduced by another module. A custom module that
    /// does not depend on the new one no longer sees the field.
    FieldMoved {
        /// The model.
        model: ModelName,
        /// The field.
        field: FieldName,
        /// Module introducing it in the source version.
        from: ModuleName,
        /// Module introducing it in the target version.
        to: ModuleName,
        /// Its first declaration in the source version.
        before: Location,
        /// Its first declaration in the target version.
        after: Location,
    },
    /// A method no definition of its model has any more.
    MethodRemoved {
        /// The model.
        model: ModelName,
        /// The method.
        method: String,
        /// Its first definition in the source version.
        before: Location,
    },
    /// A method the model did not have in the source version. Custom code
    /// that defines a method of that name now overrides it, knowingly or not.
    MethodAdded {
        /// The model.
        model: ModelName,
        /// The method.
        method: String,
        /// Its first definition in the target version.
        after: Location,
    },
    /// The parameters of the method's first definition changed, so an
    /// override written for the source version may no longer match.
    MethodParamsChanged {
        /// The model.
        model: ModelName,
        /// The method.
        method: String,
        /// Parameters in the source version.
        from: Vec<String>,
        /// Parameters in the target version.
        to: Vec<String>,
        /// The first definition in the source version.
        before: Location,
        /// The first definition in the target version.
        after: Location,
    },
    /// What a module's definition of the method does changed: its
    /// decorators, parameter defaults or body. Code that overrides or relies
    /// on it may behave differently (PTX-P403).
    MethodBodyChanged {
        /// The model.
        model: ModelName,
        /// The method.
        method: String,
        /// The module whose definition changed.
        module: ModuleName,
        /// The definition in the source version.
        before: Location,
        /// The definition in the target version.
        after: Location,
    },
}

impl Change {
    /// The model the change is about; `None` for a module.
    pub fn model(&self) -> Option<&ModelName> {
        match self {
            Self::ModuleRemoved { .. } => None,
            Self::ModelRemoved { model, .. }
            | Self::ModelAdded { model, .. }
            | Self::ParentAdded { model, .. }
            | Self::ParentRemoved { model, .. }
            | Self::FieldRemoved { model, .. }
            | Self::FieldAdded { model, .. }
            | Self::FieldStorageChanged { model, .. }
            | Self::SelectionKeysRemoved { model, .. }
            | Self::FieldTypeChanged { model, .. }
            | Self::FieldComodelChanged { model, .. }
            | Self::FieldMoved { model, .. }
            | Self::MethodRemoved { model, .. }
            | Self::MethodAdded { model, .. }
            | Self::MethodParamsChanged { model, .. }
            | Self::MethodBodyChanged { model, .. } => Some(model),
        }
    }
}
