//! Symbols extracted from Odoo source code, exactly as written: models,
//! fields and methods. Version-specific Odoo rules live in [`crate::resolve`].

use crate::field_type::FieldType;
use std::fmt;
use std::num::NonZeroU32;
use std::path::PathBuf;

/// A position in a source file.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Location {
    /// Path of the file, relative to the addons folder it was read from.
    pub file: PathBuf,
    /// Line number, starting at 1.
    pub line: NonZeroU32,
}

impl Location {
    /// Creates a location from a file path and a line number.
    pub fn new(file: impl Into<PathBuf>, line: NonZeroU32) -> Self {
        Self {
            file: file.into(),
            line,
        }
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.file.display(), self.line)
    }
}

/// Technical name of an Odoo model, for example `sale.order.line`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModelName(String);

impl ModelName {
    /// Creates a model name.
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// Returns the name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModelName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Name of a field on a model, for example `product_uom_id`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FieldName(String);

impl FieldName {
    /// Creates a field name.
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// Returns the name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FieldName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A field declared on a model, for example `partner_id = fields.Many2one("res.partner")`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDef {
    /// Field name.
    pub name: FieldName,
    /// Field type as written after `fields.`, for example `Many2one`.
    pub field_type: FieldType,
    /// Target model of a relational field (`Many2one`, `One2many`, `Many2many`).
    /// `None` when the declaration does not name one, for example a
    /// redefinition `fields.Many2one(ondelete="cascade")` that keeps the
    /// comodel of the field it overrides.
    pub comodel: Option<ModelName>,
    /// Where the field is declared.
    pub location: Location,
}

/// A method defined on a model class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodDef {
    /// Method name.
    pub name: String,
    /// Parameter names in declaration order, without the receiver (`self` or
    /// `cls`); `*args` and `**kwargs` keep their stars.
    pub params: Vec<String>,
    /// True when the method body calls `super()`.
    pub calls_super: bool,
    /// Where the method is defined.
    pub location: Location,
}

/// How `_inherit` is written in a class body. The string form and the list
/// form are kept apart because Odoo 19 treats them differently when `_name`
/// is missing (see [`crate::resolve::model_name`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Inherit {
    /// No `_inherit` attribute.
    #[default]
    Absent,
    /// `_inherit = "sale.order"`.
    Single(ModelName),
    /// `_inherit = ["mail.thread", "mail.activity.mixin"]`, even with one item.
    List(Vec<ModelName>),
}

impl Inherit {
    /// Parent model names in declaration order.
    pub fn parents(&self) -> &[ModelName] {
        match self {
            Self::Absent => &[],
            Self::Single(parent) => std::slice::from_ref(parent),
            Self::List(parents) => parents,
        }
    }
}

/// One `_inherits` entry: the parent model and the `Many2one` field that links
/// to it, for example `{"res.partner": "partner_id"}` on `res.users`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delegation {
    /// The model whose fields are delegated.
    pub parent: ModelName,
    /// The `Many2one` field pointing to the parent record.
    pub field: FieldName,
}

/// A model class as written in Python.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelDef {
    /// Python class name.
    pub class_name: String,
    /// Value of `_name`, when the class sets it.
    pub name: Option<ModelName>,
    /// Value of `_inherit`, as written.
    pub inherit: Inherit,
    /// Entries of `_inherit` (delegation inheretence), in declaration order.
    pub inherits: Vec<Delegation>,
    /// Fields declared in the class body, in declaration order.
    pub fields: Vec<FieldDef>,
    /// Methods defined in the class body, in declaration order.
    pub methods: Vec<MethodDef>,
    /// Where the class is defined.
    pub location: Location,
}

impl ModelDef {
    /// Parent models listed in `_inherit`, in declaration order.
    pub fn parents(&self) -> &[ModelName] {
        self.inherit.parents()
    }

    /// Finds a field declared in this class by name. When the class body
    /// declares the same name more than once, the last declaration wins, as in Python.
    pub fn field(&self, name: &str) -> Option<&FieldDef> {
        self.fields
            .iter()
            .rev()
            .find(|field| field.name.as_str() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(number: u32) -> NonZeroU32 {
        NonZeroU32::new(number).expect("test line numbers start at 1")
    }

    fn char_field(name: &str, at_line: u32) -> FieldDef {
        FieldDef {
            name: FieldName::new(name),
            field_type: FieldType::Char,
            comodel: None,
            location: Location::new("models/test.py", line(at_line)),
        }
    }

    #[test]
    fn location_displays_as_file_and_line() {
        let location = Location::new("models/sale_order.py", line(42));
        assert_eq!(location.to_string(), "models/sale_order.py:42");
    }

    #[test]
    fn parents_of_every_inherit_form() {
        assert!(Inherit::Absent.parents().is_empty());
        let single = Inherit::Single(ModelName::new("sale.order"));
        assert_eq!(single.parents(), &[ModelName::new("sale.order")]);
        let list = Inherit::List(vec![
            ModelName::new("mail.thread"),
            ModelName::new("mail.activity.mixin"),
        ]);
        assert_eq!(list.parents().len(), 2);
    }

    #[test]
    fn field_lookup_returns_last_declaration() {
        let class = ModelDef {
            class_name: "SaleOrderLine".to_string(),
            name: None,
            inherit: Inherit::Single(ModelName::new("sale.order.line")),
            inherits: vec![],
            fields: vec![
                char_field("customer_ref", 8),
                char_field("customer_ref", 12),
            ],
            methods: vec![],
            location: Location::new("models/test.py", line(5)),
        };
        let found_at = class
            .field("customer_ref")
            .map(|field| field.location.line.get());
        assert_eq!(found_at, Some(12));
        assert!(class.field("tax_id").is_none());
    }
}
