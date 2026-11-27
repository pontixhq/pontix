//! Field declarations in a model class body: `name = fields.X(...)`.

use pontix_core::{FieldAttrs, FieldType, ModelName, Written};
use ruff_python_ast::{Expr, ExprCall};

/// What the value of a class attribute declares, as far as fields go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Field {
    /// Written `fields.X(...)`.
    Declared {
        /// The `X` in `fields.X(...)`.
        field_type: FieldType,
        /// The comodel, for relational types only.
        comodel: Comodel,
        /// The arguments that decide how it is stored.
        attrs: FieldAttrs,
    },
    /// Written with the name of a core field type but not as `fields.X(...)`,
    /// for example `Char(...)`, `f.Char(...)` or `fields.Char` without a call.
    /// Pontix does not guess what such a name refers to.
    Unrecognized,
}

/// The comodel of a relational field, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Comodel {
    /// Not a relational type, or a relational field that does not name its
    /// comodel, for example a redefinition `fields.Many2one(ondelete="cascade")`
    /// that keeps the comodel of the field it overrides.
    Absent,
    /// The first positional argument or `comodel_name=`, as a string literal.
    Named(ModelName),
    /// Given, but not as a string literal.
    Unreadable,
}

/// Reads the value assigned to a class attribute. `None` means the value has
/// nothing to do with fields, for example `models.Constraint(...)` or
/// `re.compile(...)`. `model_name` is the `_name` assigned above, which a
/// comodel may refer to, as in `parent_id = fields.Many2one(_name)`.
pub(super) fn field(value: &Expr, model_name: Option<&ModelName>) -> Option<Field> {
    match value {
        Expr::Call(call) => match &*call.func {
            Expr::Attribute(attribute) if is_fields_module(&attribute.value) => {
                let field_type = FieldType::from_name(attribute.attr.as_str());
                let comodel = if field_type.is_relational() {
                    comodel(call, model_name)
                } else {
                    Comodel::Absent
                };
                let attrs = attrs(call, &field_type);
                Some(Field::Declared {
                    field_type,
                    comodel,
                    attrs,
                })
            }
            func if names_core_field_type(func) => Some(Field::Unrecognized),
            _ => None,
        },
        Expr::Attribute(attribute) if is_fields_module(&attribute.value) => {
            Some(Field::Unrecognized)
        }
        _ => None,
    }
}

/// `fields` or `odoo.fields`.
fn is_fields_module(expr: &Expr) -> bool {
    match expr {
        Expr::Name(name) => name.id.as_str() == "fields",
        Expr::Attribute(attribute) => attribute.attr.as_str() == "fields",
        _ => false,
    }
}

/// `Char` or `anything.Char`, for any field type Odoo core defines.
fn names_core_field_type(func: &Expr) -> bool {
    let name = match func {
        Expr::Name(name) => name.id.as_str(),
        Expr::Attribute(attribute) => attribute.attr.as_str(),
        _ => return false,
    };
    !matches!(FieldType::from_name(name), FieldType::Other(_))
}

/// The comodel is the first positional argument, or `comodel_name=`.
fn comodel(call: &ExprCall, model_name: Option<&ModelName>) -> Comodel {
    let written = call.arguments.args.first().or_else(|| {
        call.arguments
            .keywords
            .iter()
            .find(|keyword| {
                keyword
                    .arg
                    .as_ref()
                    .is_some_and(|arg| arg.as_str() == "comodel_name")
            })
            .map(|keyword| &keyword.value)
    });
    match written {
        None => Comodel::Absent,
        Some(Expr::Name(name)) if name.id.as_str() == "_name" => model_name
            .cloned()
            .map_or(Comodel::Unreadable, Comodel::Named),
        Some(Expr::StringLiteral(literal)) => {
            Comodel::Named(ModelName::new(literal.value.to_str()))
        }
        Some(_) => Comodel::Unreadable,
    }
}

/// Reads `store=`, `compute=`, `related=`, `attachment=`, `selection=` and
/// `selection_add=`. A `Selection` field also takes its selection as the
/// first positional argument. With `**kwargs` in the call, any argument the
/// call does not write explicitly may come from there, so it is unreadable.
fn attrs(call: &ExprCall, field_type: &FieldType) -> FieldAttrs {
    let mut attrs = FieldAttrs {
        store: argument(call, "store", boolean),
        compute: argument(call, "compute", method_name),
        related: argument(call, "related", |value| string(value).map(str::to_string)),
        attachment: argument(call, "attachment", boolean),
        selection: argument(call, "selection", selection_keys),
        selection_add: argument(call, "selection_add", selection_keys),
    };
    if *field_type == FieldType::Selection
        && !attrs.selection.is_given()
        && let Some(first) = call.arguments.args.first()
    {
        attrs.selection = selection_keys(first).map_or(Written::Unreadable, Written::Value);
    }
    attrs
}

/// The keyword argument `name`, read with `how`.
fn argument<T>(call: &ExprCall, name: &str, how: impl Fn(&Expr) -> Option<T>) -> Written<T> {
    let keywords = &call.arguments.keywords;
    match keywords
        .iter()
        .find(|keyword| keyword.arg.as_ref().is_some_and(|arg| arg.as_str() == name))
    {
        Some(keyword) => how(&keyword.value).map_or(Written::Unreadable, Written::Value),
        None if keywords.iter().any(|keyword| keyword.arg.is_none()) => Written::Unreadable,
        None => Written::Absent,
    }
}

fn boolean(expr: &Expr) -> Option<bool> {
    match expr {
        Expr::BooleanLiteral(literal) => Some(literal.value),
        _ => None,
    }
}

fn string(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::StringLiteral(literal) => Some(literal.value.to_str()),
        _ => None,
    }
}

/// `compute="_compute_total"` or `compute=_compute_total`.
fn method_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Name(name) => Some(name.id.as_str().to_string()),
        other => string(other).map(str::to_string),
    }
}

/// The keys of a list or tuple of `(key, label)` pairs; `selection_add` may
/// also hold `(key,)` to place an existing key.
fn selection_keys(expr: &Expr) -> Option<Vec<String>> {
    let items = match expr {
        Expr::List(list) => &list.elts,
        Expr::Tuple(tuple) => &tuple.elts,
        _ => return None,
    };
    items
        .iter()
        .map(|item| {
            let pair = match item {
                Expr::Tuple(tuple) => &tuple.elts,
                Expr::List(list) => &list.elts,
                _ => return None,
            };
            pair.first().and_then(string).map(str::to_string)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ruff_python_parser::parse_expression;

    fn read(source: &str) -> Option<Field> {
        let parsed = parse_expression(source).expect("test source must be a valid expression");
        field(&parsed.syntax().body, None)
    }

    fn declared(field_type: FieldType, comodel: Comodel) -> Option<Field> {
        with_attrs(field_type, comodel, FieldAttrs::default())
    }

    fn with_attrs(field_type: FieldType, comodel: Comodel, attrs: FieldAttrs) -> Option<Field> {
        Some(Field::Declared {
            field_type,
            comodel,
            attrs,
        })
    }

    fn attrs_of(source: &str) -> FieldAttrs {
        match read(source) {
            Some(Field::Declared { attrs, .. }) => attrs,
            other => panic!("{source} is not a field declaration: {other:?}"),
        }
    }

    fn keys(values: &[&str]) -> Written<Vec<String>> {
        Written::Value(values.iter().map(|key| key.to_string()).collect())
    }

    #[test]
    fn reads_simple_fields() {
        assert_eq!(
            read("fields.Char(string='Name', required=True)"),
            declared(FieldType::Char, Comodel::Absent)
        );
        assert_eq!(
            read("odoo.fields.Integer()"),
            declared(FieldType::Integer, Comodel::Absent)
        );
    }

    #[test]
    fn reads_comodel_from_position_or_keyword() {
        let partner = || Comodel::Named(ModelName::new("res.partner"));
        assert_eq!(
            read("fields.Many2one('res.partner', ondelete='cascade')"),
            declared(FieldType::Many2one, partner())
        );
        assert_eq!(
            read("fields.Many2many(comodel_name='res.partner')"),
            declared(FieldType::Many2many, partner())
        );
        assert_eq!(
            read("fields.One2many('res.partner', 'parent_id')"),
            declared(FieldType::One2many, partner())
        );
    }

    #[test]
    fn redefinition_without_comodel_is_absent_not_unreadable() {
        assert_eq!(
            read("fields.Many2one(ondelete='cascade')"),
            declared(FieldType::Many2one, Comodel::Absent)
        );
        assert_eq!(
            read("fields.Many2one(related='order_id.partner_id')"),
            with_attrs(
                FieldType::Many2one,
                Comodel::Absent,
                FieldAttrs {
                    related: Written::Value("order_id.partner_id".to_string()),
                    ..FieldAttrs::default()
                }
            )
        );
    }

    #[test]
    fn comodel_that_is_not_a_string_is_unreadable() {
        assert_eq!(
            read("fields.Many2one(PARTNER_MODEL)"),
            declared(FieldType::Many2one, Comodel::Unreadable)
        );
        assert_eq!(
            read("fields.Many2one(comodel_name=_get_model())"),
            declared(FieldType::Many2one, Comodel::Unreadable)
        );
    }

    #[test]
    fn comodel_may_refer_to_name_assigned_above() {
        let parsed = parse_expression("fields.Many2one(_name, compute='_compute_root')")
            .expect("test source must be a valid expression");
        let category = ModelName::new("test_new_api.category");
        let computed = || FieldAttrs {
            compute: Written::Value("_compute_root".to_string()),
            ..FieldAttrs::default()
        };
        assert_eq!(
            field(&parsed.syntax().body, Some(&category)),
            with_attrs(
                FieldType::Many2one,
                Comodel::Named(category.clone()),
                computed()
            )
        );
        assert_eq!(
            field(&parsed.syntax().body, None),
            with_attrs(FieldType::Many2one, Comodel::Unreadable, computed())
        );
    }

    #[test]
    fn non_relational_types_never_have_a_comodel() {
        assert_eq!(
            read("fields.Selection('_selection_values')"),
            with_attrs(
                FieldType::Selection,
                Comodel::Absent,
                FieldAttrs {
                    selection: Written::Unreadable,
                    ..FieldAttrs::default()
                }
            )
        );
    }

    #[test]
    fn reads_the_arguments_that_decide_storage() {
        let attrs = attrs_of("fields.Monetary(compute='_compute_total', store=True)");
        assert_eq!(attrs.compute, Written::Value("_compute_total".to_string()));
        assert_eq!(attrs.store, Written::Value(true));
        assert_eq!(
            attrs_of("fields.Char(compute=_compute_name)").compute,
            Written::Value("_compute_name".to_string())
        );
        assert_eq!(
            attrs_of("fields.Binary(attachment=False)").attachment,
            Written::Value(false)
        );
        assert_eq!(
            attrs_of("fields.Char(store=STORED)").store,
            Written::Unreadable
        );
        assert_eq!(
            attrs_of("fields.Char(compute=lambda self: None)").compute,
            Written::Unreadable
        );
    }

    #[test]
    fn reads_selection_keys_from_either_place() {
        assert_eq!(
            attrs_of("fields.Selection([('draft', 'Draft'), ('done', 'Done')], default='draft')")
                .selection,
            keys(&["draft", "done"])
        );
        assert_eq!(
            attrs_of("fields.Selection(selection=[('a', 'A')])").selection,
            keys(&["a"])
        );
        assert_eq!(
            attrs_of("fields.Selection(selection_add=[('sent', 'Sent'), ('done',)])").selection_add,
            keys(&["sent", "done"])
        );
        assert_eq!(
            attrs_of("fields.Selection([(KEY, 'Label')])").selection,
            Written::Unreadable
        );
    }

    #[test]
    fn keyword_splat_makes_unwritten_arguments_unreadable() {
        let attrs = attrs_of("fields.Char(store=False, **COMMON)");
        assert_eq!(attrs.store, Written::Value(false));
        assert_eq!(attrs.compute, Written::Unreadable);
    }

    #[test]
    fn unknown_field_class_is_kept_as_written() {
        assert_eq!(
            read("fields.ColorPicker()"),
            declared(FieldType::Other("ColorPicker".to_string()), Comodel::Absent)
        );
    }

    #[test]
    fn core_type_names_outside_fields_are_unrecognized() {
        assert_eq!(read("Char(string='Name')"), Some(Field::Unrecognized));
        assert_eq!(read("f.Many2one('res.partner')"), Some(Field::Unrecognized));
        assert_eq!(read("fields.Char"), Some(Field::Unrecognized));
    }

    #[test]
    fn other_values_are_not_fields() {
        for source in [
            "models.Constraint('UNIQUE(name)', 'Name must be unique.')",
            "re.compile(r'\\d+')",
            "'res.partner'",
            "SomeHelper()",
        ] {
            assert_eq!(read(source), None, "{source}");
        }
    }
}
