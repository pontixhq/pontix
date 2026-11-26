//! Symbols extracted from Odoo source code: models, fields, and methods.

use std::fmt;
use std::path::PathBuf;

/// A position in a source file.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Location {
    /// Path of the file, relative to the addons folder it was read from.
    pub file: PathBuf,
    /// Line number, starting at 1.
    pub line: u32,
}

impl Location {
    /// Creates a location from a file path and a 1-based line number.
    pub fn new(file: impl Into<PathBuf>, line: u32) -> Self {
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

/// Name of a field on a mode, for example `product_uom_id`.
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

/// A field declared on a model, for example `partner_id = fields.Many2One("res.partner")`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDef {
    /// Field name.
    pub name: FieldName,
    /// Field type as written after `fields.`, for example `Many2One`.
    pub field_type: String,
    /// Target model of a relational field (`Many2One`, `One2Many`, `Many2Many`).
    pub comodel: Option<ModelName>,
    /// Where the field is declared.
    pub location: Location,
}

/// A method defined on a model class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodDef {
    /// Method name.
    pub name: String,
    /// Parameter names, without `self`.
    pub params: Vec<String>,
    /// True when the method body calls `super()`.
    pub calls_super: bool,
    /// Where the method is defined.
    pub location: Location,
}

/// A model class: a new model (`_name`) or an extension of one (`_inherit`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelDef {
    /// Python class name.
    pub class_name: String,
    /// Value of `_name`, when the class sets it.
    pub name: Option<ModelName>,
    /// Value of `_inherit`; a single string becomes a list of one.
    pub inherit: Vec<ModelName>,
    /// Fields declared in the class body.
    pub fields: Vec<FieldDef>,
    /// Methods defined in the class body.
    pub methods: Vec<MethodDef>,
    /// Where the class is defined.
    pub location: Location,
}

impl ModelDef {
    /// The model this class defines or extends: `_name` when set,
    /// otherwise the first `_inherit`.
    pub fn target(&self) -> Option<&ModelName> {
        self.name.as_ref().or_else(|| self.inherit.first())
    }

    /// Finds a field declared in this class by name.
    pub fn field(&self, name: &str) -> Option<&FieldDef> {
        self.fields.iter().find(|field| field.name.as_str() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sale_line_extension() -> ModelDef {
        ModelDef {
            class_name: "SaleOrderLine".to_string(),
            name: None,
            inherit: vec![ModelName::new("sale.order.line")],
            fields: vec![FieldDef {
                name: FieldName::new("customer_ref"),
                field_type: "Char".to_string(),
                comodel: None,
                location: Location::new("models/sale_order_line.py", 8),
            }],
            methods: vec![],
            location: Location::new("models/sale_order_line.py", 5),
        }
    }

    #[test]
    fn location_display_as_file_and_line() {
        let location = Location::new("models/sale_order.py", 42);
        assert_eq!(location.to_string(), "models/sale_order.py:42");
    }

    #[test]
    fn target_is_inherit_when_name_is_missing() {
        let model = sale_line_extension();
        assert_eq!(model.target(), Some(&ModelName::new("sale.order.line")));
    }

    #[test]
    fn target_prefers_name_over_inherit() {
        let mut model = sale_line_extension();
        model.name = Some(ModelName::new("sale.order.line.custom"));
        assert_eq!(
            model.target(),
            Some(&ModelName::new("sale.order.line.custom"))
        );
    }

    #[test]
    fn field_lookup_by_name() {
        let model = sale_line_extension();
        let field_type = model.field("customer_ref").map(|f| f.field_type.as_str());
        assert_eq!(field_type, Some("Char"));
        assert!(model.field("tax_id").is_none());
    }
}
