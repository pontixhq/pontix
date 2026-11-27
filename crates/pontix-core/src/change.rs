//! What changed between two Odoo versions: the `ChangeSet`.
//!
//! Each change carries where it can be seen: the definition in the source
//! version, the one in the target version, or both. A removal is shown by
//! its source definition alone.

use crate::field_type::FieldType;
use crate::manifest::ModuleName;
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
    /// A field no definition of its model declares any more.
    FieldRemoved {
        /// The model.
        model: ModelName,
        /// The field.
        field: FieldName,
        /// Its first declaration in the source version.
        before: Location,
    },
    /// A field the model did not have in the source version.
    FieldAdded {
        /// The model.
        model: ModelName,
        /// The field.
        field: FieldName,
        /// Its first declaration in the target version.
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
            | Self::FieldRemoved { model, .. }
            | Self::FieldAdded { model, .. }
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
