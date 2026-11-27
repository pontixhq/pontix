//! Version-aware resolution of Odoo model classes.
//!
//! Odoo decides a class's model name, and whether it extends an existing
//! model, with rules that changed between versions. Each rule below cites
//! the core source it follows.

use std::borrow::Cow;

use crate::symbol::{Inherit, ModelDef, ModelName};
use crate::version::OdooVersion;

/// How a model class relates to existing models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelKind {
    /// No parents: a brand-new model.
    New,
    /// The model name is one of its parents: the class adds to an existing
    /// model in place.
    Extension,
    /// The model name differs from every parent: a new model built from them.
    Derived,
}

/// The model name Odoo gives this class in `version`.
pub fn model_name(def: &ModelDef, version: OdooVersion) -> Cow<'_, ModelName> {
    if let Some(name) = &def.name {
        return Cow::Borrowed(name);
    }
    match (version, &def.inherit) {
        // Both versions: a string `_inherit` is the default `_name`.
        (OdooVersion::V18 | OdooVersion::V19, Inherit::Single(parent)) => Cow::Borrowed(parent),
        // 18.0, odoo/models.py, MetaModel.__new__:
        //   attrs['_name'] = inherit[0] if len(inherit) == 1 else name
        (OdooVersion::V18, Inherit::List(parents)) => match parents.as_slice() {
            [only] => Cow::Borrowed(only),
            _ => Cow::Owned(ModelName::new(def.class_name.as_str())),
        },
        (OdooVersion::V18, Inherit::Absent) => Cow::Owned(ModelName::new(def.class_name.as_str())),
        // 19.0, odoo/orm/models.py:241-248: only a string `_inherit` becomes
        // `_name`; otherwise the name is derived from the class name.
        (OdooVersion::V19, Inherit::List(_) | Inherit::Absent) => {
            Cow::Owned(ModelName::new(name_from_class(&def.class_name)))
        }
    }
}

/// How the class relates to existing models in `version`.
///
/// Both versions treat the class as an extension when its model name is one
/// of its parents (19.0: odoo/orm/model_classes.py:173, 18.0: odoo/models.py:739).
pub fn model_kind(def: &ModelDef, version: OdooVersion) -> ModelKind {
    let parents = def.parents();
    let name = model_name(def, version);
    if parents.contains(&name) {
        ModelKind::Extension
    } else if parents.is_empty() {
        ModelKind::New
    } else {
        ModelKind::Derived
    }
}

/// Derives a model name from a Python class name as Odoo 19 does when `_name`
/// is missing: a dot before every ASCII uppercase letter that follows a
/// character other than `_`, then lowercase (odoo/orm/models.py:248).
pub fn name_from_class(class_name: &str) -> String {
    let mut name = String::with_capacity(class_name.len() + 4);
    let mut previous: Option<char> = None;
    for ch in class_name.chars() {
        if ch.is_ascii_uppercase() && previous.is_some_and(|prev| prev != '_') {
            name.push('.');
        }
        name.extend(ch.to_lowercase());
        previous = Some(ch);
    }
    name
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use super::*;
    use crate::symbol::Location;

    fn class(class_name: &str, name: Option<&str>, inherit: Inherit) -> ModelDef {
        ModelDef {
            class_name: class_name.to_string(),
            name: name.map(ModelName::new),
            inherit,
            inherits: vec![],
            fields: vec![],
            methods: vec![],
            location: Location::new("models/test.py", NonZeroU32::MIN),
        }
    }

    fn list(parents: &[&str]) -> Inherit {
        Inherit::List(
            parents
                .iter()
                .map(|parent| ModelName::new(*parent))
                .collect(),
        )
    }

    #[test]
    fn string_inherit_extends_in_both_versions() {
        let def = class(
            "Anything",
            None,
            Inherit::Single(ModelName::new("sale.order.line")),
        );
        for version in [OdooVersion::V18, OdooVersion::V19] {
            assert_eq!(model_name(&def, version).as_str(), "sale.order.line");
            assert_eq!(model_kind(&def, version), ModelKind::Extension);
        }
    }

    #[test]
    fn one_item_list_extends_in_18_but_creates_a_new_model_in_19() {
        let def = class("SaleOrderLineCustom", None, list(&["sale.order.line"]));
        assert_eq!(
            model_name(&def, OdooVersion::V18).as_str(),
            "sale.order.line"
        );
        assert_eq!(model_kind(&def, OdooVersion::V18), ModelKind::Extension);
        assert_eq!(
            model_name(&def, OdooVersion::V19).as_str(),
            "sale.order.line.custom"
        );
        assert_eq!(model_kind(&def, OdooVersion::V19), ModelKind::Derived);
    }

    #[test]
    fn one_item_list_still_extends_in_19_when_the_class_name_matches() {
        let def = class("SaleOrderLine", None, list(&["sale.order.line"]));
        assert_eq!(model_kind(&def, OdooVersion::V19), ModelKind::Extension);
    }

    #[test]
    fn explicit_name_equal_to_a_parent_is_an_extension() {
        let def = class(
            "SaleOrder",
            Some("sale.order"),
            list(&["sale.order", "mail.thread"]),
        );
        assert_eq!(model_kind(&def, OdooVersion::V19), ModelKind::Extension);
    }

    #[test]
    fn explicit_name_different_from_parents_is_derived() {
        let def = class(
            "Custom",
            Some("custom.sale.order.line"),
            list(&["sale.order.line"]),
        );
        assert_eq!(model_kind(&def, OdooVersion::V19), ModelKind::Derived);
    }

    #[test]
    fn missing_name_and_inherit_falls_back_to_the_class_name() {
        let def = class("LibraryBook", None, Inherit::Absent);
        assert_eq!(model_name(&def, OdooVersion::V18).as_str(), "LibraryBook");
        assert_eq!(model_name(&def, OdooVersion::V19).as_str(), "library.book");
        assert_eq!(model_kind(&def, OdooVersion::V19), ModelKind::New);
    }

    #[test]
    fn name_from_class_follows_the_odoo_19_regex() {
        assert_eq!(name_from_class("SaleOrderLine"), "sale.order.line");
        assert_eq!(name_from_class("Res_Partner"), "res_partner");
        assert_eq!(name_from_class("HTTPHandler"), "h.t.t.p.handler");
    }
}
