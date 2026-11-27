//! Field types defined by Odoo core, as written after `fields.`.

use std::fmt;

/// Type of a field declaration, for example `Many2one` in `fields.Many2one(...)`.
///
/// The named variants are every field class used by Odoo 18 and 19 core addons
/// (survey of 27 Nov 2026: the same 22 in both versions).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FieldType {
    /// `fields.Binary`.
    Binary,
    /// `fields.Boolean`.
    Boolean,
    /// `fields.Char`.
    Char,
    /// `fields.Date`.
    Date,
    /// `fields.Datetime`.
    Datetime,
    /// `fields.Float`.
    Float,
    /// `fields.Html`.
    Html,
    /// `fields.Id`.
    Id,
    /// `fields.Image`.
    Image,
    /// `fields.Integer`.
    Integer,
    /// `fields.Json`.
    Json,
    /// `fields.Many2many`.
    Many2many,
    /// `fields.Many2one`.
    Many2one,
    /// `fields.Many2oneReference`.
    Many2oneReference,
    /// `fields.Monetary`.
    Monetary,
    /// `fields.One2many`.
    One2many,
    /// `fields.Properties`.
    Properties,
    /// `fields.PropertiesDefinition`.
    PropertiesDefinition,
    /// `fields.Reference`.
    Reference,
    /// `fields.Selection`.
    Selection,
    /// `fields.Serialized`.
    Serialized,
    /// `fields.Text`.
    Text,
    /// A field class that Odoo core does not define, for example one provided
    /// by a custom module. The name is kept exactly as written.
    Other(String),
}

impl FieldType {
    /// Reads the name written after `fields.`; names core does not define
    /// become [`FieldType::Other`].
    pub fn from_name(name: &str) -> Self {
        match name {
            "Binary" => Self::Binary,
            "Boolean" => Self::Boolean,
            "Char" => Self::Char,
            "Date" => Self::Date,
            "Datetime" => Self::Datetime,
            "Float" => Self::Float,
            "Html" => Self::Html,
            "Id" => Self::Id,
            "Image" => Self::Image,
            "Integer" => Self::Integer,
            "Json" => Self::Json,
            "Many2many" => Self::Many2many,
            "Many2one" => Self::Many2one,
            "Many2oneReference" => Self::Many2oneReference,
            "Monetary" => Self::Monetary,
            "One2many" => Self::One2many,
            "Properties" => Self::Properties,
            "PropertiesDefinition" => Self::PropertiesDefinition,
            "Reference" => Self::Reference,
            "Selection" => Self::Selection,
            "Serialized" => Self::Serialized,
            "Text" => Self::Text,
            other => Self::Other(other.to_string()),
        }
    }

    /// The name as written after `fields.`.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Binary => "Binary",
            Self::Boolean => "Boolean",
            Self::Char => "Char",
            Self::Date => "Date",
            Self::Datetime => "Datetime",
            Self::Float => "Float",
            Self::Html => "Html",
            Self::Id => "Id",
            Self::Image => "Image",
            Self::Integer => "Integer",
            Self::Json => "Json",
            Self::Many2many => "Many2many",
            Self::Many2one => "Many2one",
            Self::Many2oneReference => "Many2oneReference",
            Self::Monetary => "Monetary",
            Self::One2many => "One2many",
            Self::Properties => "Properties",
            Self::PropertiesDefinition => "PropertiesDefinition",
            Self::Reference => "Reference",
            Self::Selection => "Selection",
            Self::Serialized => "Serialized",
            Self::Text => "Text",
            Self::Other(name) => name,
        }
    }

    /// True for fields whose value points to records of another model
    /// (`Many2one`, `One2many`, `Many2many`).
    pub fn is_relational(&self) -> bool {
        matches!(self, Self::Many2one | Self::One2many | Self::Many2many)
    }
}

impl fmt::Display for FieldType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CORE_TYPES: [&str; 22] = [
        "Binary",
        "Boolean",
        "Char",
        "Date",
        "Datetime",
        "Float",
        "Html",
        "Id",
        "Image",
        "Integer",
        "Json",
        "Many2many",
        "Many2one",
        "Many2oneReference",
        "Monetary",
        "One2many",
        "Properties",
        "PropertiesDefinition",
        "Reference",
        "Selection",
        "Serialized",
        "Text",
    ];

    #[test]
    fn every_core_type_round_trips() {
        for name in CORE_TYPES {
            let field_type = FieldType::from_name(name);
            assert!(
                !matches!(field_type, FieldType::Other(_)),
                "{name} should be known"
            );
            assert_eq!(field_type.as_str(), name);
        }
    }

    #[test]
    fn unknown_type_is_kept_as_written() {
        let field_type = FieldType::from_name("ColorPicker");
        assert_eq!(field_type, FieldType::Other("ColorPicker".to_string()));
        assert_eq!(field_type.to_string(), "ColorPicker");
    }

    #[test]
    fn only_three_types_are_relational() {
        let relational: Vec<&str> = CORE_TYPES
            .into_iter()
            .filter(|name| FieldType::from_name(name).is_relational())
            .collect();
        assert_eq!(relational, ["Many2many", "Many2one", "One2many"]);
    }
}
