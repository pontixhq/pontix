//! QWeb expressions: the Python in `t-field`, `t-esc`, `t-out`, `t-if`,
//! `t-foreach`, `t-value` and `t-att-*`, and the `{{ }}` / `#{ }` pieces
//! of `t-attf-*`, read for the attribute paths they follow from a name,
//! such as `doc.partner_id.name`.

use ruff_python_ast::Expr;
use ruff_python_parser::parse_expression;

/// A name followed by attributes, as written: `doc.partner_id.name` is
/// `doc` then `partner_id`, `name`. A method called on the way ends the
/// chain before it: `doc.partner_id._get_name()` is `doc` then
/// `partner_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chain {
    /// The name the chain starts from.
    pub root: String,
    /// The attributes after it.
    pub attributes: Vec<String>,
}

/// What a QWeb expression reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QwebExpr {
    /// Every chain it follows, in order.
    pub chains: Vec<Chain>,
    /// The chain that is its whole value, when it is one: `doc.line_ids`,
    /// also through calls that keep the records' model (`filtered`,
    /// `sorted`, `sudo`, ...) or follow fields (`mapped('line_ids')`).
    pub value: Option<Chain>,
}

/// Methods that give records of the same model.
const SAME_MODEL: [&str; 9] = [
    "filtered",
    "filtered_domain",
    "sorted",
    "sudo",
    "with_context",
    "with_company",
    "with_user",
    "with_env",
    "exists",
];

/// Reads an expression; `None` when it is not Python.
pub fn read_expression(expression: &str) -> Option<QwebExpr> {
    let parsed = parse_expression(expression.trim()).ok()?;
    let body = &parsed.syntax().body;
    let mut chains = Vec::new();
    collect(body, &mut chains);
    Some(QwebExpr {
        chains,
        value: value(body),
    })
}

/// The `{{ ... }}` and `#{ ... }` pieces of a `t-attf-*` value.
pub fn format_expressions(template: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = template;
    loop {
        let start = match (rest.find("{{"), rest.find("#{")) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => break,
        };
        let close = if rest[start..].starts_with("{{") {
            "}}"
        } else {
            "}"
        };
        let open = 2;
        let Some(end) = rest[start + open..].find(close) else {
            break;
        };
        found.push(rest[start + open..start + open + end].trim());
        rest = &rest[start + open + end + close.len()..];
    }
    found
}

/// The chain an expression is, when it is one.
fn value(expr: &Expr) -> Option<Chain> {
    match expr {
        Expr::Name(name) => Some(Chain {
            root: name.id.as_str().to_string(),
            attributes: Vec::new(),
        }),
        Expr::Attribute(attribute) => {
            let mut chain = value(&attribute.value)?;
            chain.attributes.push(attribute.attr.as_str().to_string());
            Some(chain)
        }
        Expr::Call(call) => {
            let Expr::Attribute(method) = &*call.func else {
                return None;
            };
            let mut chain = value(&method.value)?;
            let name = method.attr.as_str();
            if SAME_MODEL.contains(&name) {
                return Some(chain);
            }
            // `mapped('line_ids.product_id')` follows those fields.
            if name == "mapped"
                && let [Expr::StringLiteral(path)] = call.arguments.args.as_ref()
            {
                chain
                    .attributes
                    .extend(path.value.to_str().split('.').map(str::to_string));
                return Some(chain);
            }
            None
        }
        _ => None,
    }
}

/// Every chain an expression follows.
fn collect(expr: &Expr, found: &mut Vec<Chain>) {
    match expr {
        Expr::Name(_) | Expr::Attribute(_) => {
            if let Some(chain) = value(expr) {
                found.push(chain);
            } else if let Expr::Attribute(attribute) = expr {
                collect(&attribute.value, found);
            }
        }
        Expr::Call(call) => {
            // The called method ends the chain; `mapped` and the like keep
            // following it.
            match value(expr) {
                Some(chain) => found.push(chain),
                None => match &*call.func {
                    Expr::Attribute(method) => collect(&method.value, found),
                    other => collect(other, found),
                },
            }
            for argument in &call.arguments.args {
                collect(argument, found);
            }
            for keyword in &call.arguments.keywords {
                collect(&keyword.value, found);
            }
        }
        Expr::BoolOp(boolean) => boolean.values.iter().for_each(|item| collect(item, found)),
        Expr::BinOp(binary) => {
            collect(&binary.left, found);
            collect(&binary.right, found);
        }
        Expr::UnaryOp(unary) => collect(&unary.operand, found),
        Expr::Compare(compare) => compare
            .operands
            .iter()
            .for_each(|item| collect(item, found)),
        Expr::If(conditional) => {
            collect(&conditional.test, found);
            collect(&conditional.body, found);
            collect(&conditional.orelse, found);
        }
        Expr::Subscript(subscript) => {
            collect(&subscript.value, found);
            collect(&subscript.slice, found);
        }
        Expr::List(list) => list.elts.iter().for_each(|item| collect(item, found)),
        Expr::Tuple(tuple) => tuple.elts.iter().for_each(|item| collect(item, found)),
        Expr::Dict(dict) => {
            for item in &dict.items {
                if let Some(key) = &item.key {
                    collect(key, found);
                }
                collect(&item.value, found);
            }
        }
        Expr::FString(_) | Expr::StringLiteral(_) | Expr::NumberLiteral(_) => {}
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chains(expression: &str) -> Vec<String> {
        read_expression(expression)
            .expect("Python")
            .chains
            .iter()
            .map(|chain| {
                std::iter::once(chain.root.as_str())
                    .chain(chain.attributes.iter().map(String::as_str))
                    .collect::<Vec<_>>()
                    .join(".")
            })
            .collect()
    }

    fn value(expression: &str) -> Option<String> {
        read_expression(expression)
            .expect("Python")
            .value
            .map(|chain| {
                std::iter::once(chain.root)
                    .chain(chain.attributes)
                    .collect::<Vec<_>>()
                    .join(".")
            })
    }

    #[test]
    fn chains_stop_before_called_methods() {
        assert_eq!(chains("doc.partner_id.name"), ["doc.partner_id.name"]);
        assert_eq!(
            chains("doc.partner_id._get_name() or line.price_unit * line.qty"),
            ["doc.partner_id", "line.price_unit", "line.qty"]
        );
        assert_eq!(
            chains("format_amount(env, doc.amount_total, doc.currency_id)"),
            [
                "format_amount",
                "env",
                "doc.amount_total",
                "doc.currency_id"
            ]
        );
        assert_eq!(
            chains("doc.line_ids.filtered(lambda l: l.qty)"),
            ["doc.line_ids"]
        );
        assert_eq!(chains("'%s' % doc.name"), ["doc.name"]);
    }

    #[test]
    fn values_follow_calls_that_keep_the_model() {
        assert_eq!(value("doc.order_line"), Some("doc.order_line".into()));
        assert_eq!(
            value("doc.order_line.filtered(lambda l: l.qty).sorted('sequence')"),
            Some("doc.order_line".into())
        );
        assert_eq!(
            value("docs.mapped('order_line.product_id')"),
            Some("docs.order_line.product_id".into())
        );
        assert_eq!(value("doc._get_lines()"), None);
        assert_eq!(value("doc.amount and doc.name"), None);
        assert!(read_expression("not ( python").is_none());
    }

    #[test]
    fn format_pieces_are_found() {
        assert_eq!(
            format_expressions("/report/{{ doc.id }}?x=#{doc.partner_id.name}&y={{a}}"),
            ["doc.id", "doc.partner_id.name", "a"]
        );
        assert!(format_expressions("plain {{ unclosed").is_empty());
    }
}
