//! Method definitions in a model class body.

use std::hash::{Hash, Hasher};

use pontix_core::ApiDecorator;
use ruff_python_ast::comparable::{ComparableExpr, ComparableStmt};
use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};
use ruff_python_ast::{Expr, Stmt, StmtFunctionDef};
use ruff_text_size::{Ranged, TextSize};

/// Parameter names in declaration order, without the receiver (`self` or
/// `cls`). `*args` and `**kwargs` keep their stars. A `@staticmethod` has no
/// receiver, so all of its parameters are kept.
pub(super) fn params(def: &StmtFunctionDef) -> Vec<String> {
    let parameters = &def.parameters;
    let receiver = usize::from(!is_staticmethod(def));
    let positional = parameters
        .posonlyargs
        .iter()
        .chain(&parameters.args)
        .skip(receiver)
        .map(|parameter| parameter.parameter.name.as_str().to_string());
    let vararg = parameters
        .vararg
        .iter()
        .map(|parameter| format!("*{}", parameter.name.as_str()));
    let keyword_only = parameters
        .kwonlyargs
        .iter()
        .map(|parameter| parameter.parameter.name.as_str().to_string());
    let kwarg = parameters
        .kwarg
        .iter()
        .map(|parameter| format!("**{}", parameter.name.as_str()));
    positional
        .chain(vararg)
        .chain(keyword_only)
        .chain(kwarg)
        .collect()
}

/// The field paths of each `@api.depends`, `@api.onchange` and
/// `@api.constrains` decorator, with where the decorator starts. Arguments
/// that are not string literals, such as `*FIELDS` or a lambda, are left out.
pub(super) fn field_paths(def: &StmtFunctionDef) -> Vec<(ApiDecorator, Vec<String>, TextSize)> {
    def.decorator_list
        .iter()
        .filter_map(|decorator| {
            let Expr::Call(call) = &decorator.expression else {
                return None;
            };
            let Expr::Attribute(attribute) = &*call.func else {
                return None;
            };
            let from_api = match &*attribute.value {
                Expr::Name(module) => module.id.as_str() == "api",
                Expr::Attribute(module) => module.attr.as_str() == "api",
                _ => false,
            };
            let kind = match attribute.attr.as_str() {
                "depends" => ApiDecorator::Depends,
                "onchange" => ApiDecorator::Onchange,
                "constrains" => ApiDecorator::Constrains,
                _ => return None,
            };
            if !from_api {
                return None;
            }
            let paths = call
                .arguments
                .args
                .iter()
                .filter_map(|argument| match argument {
                    Expr::StringLiteral(literal) => Some(literal.value.to_str().to_string()),
                    _ => None,
                })
                .collect();
            Some((kind, paths, decorator.start()))
        })
        .collect()
}

/// True when the method body calls `super()` or `super(Class, self)`.
/// Functions and classes nested in the body are not counted: their `super()`
/// belongs to them, not to the method.
pub(super) fn calls_super(def: &StmtFunctionDef) -> bool {
    let mut finder = SuperFinder { found: false };
    finder.visit_body(&def.body);
    finder.found
}

/// A fingerprint of what the method does: its decorators, parameter names
/// with their default values, and body. Positions, formatting and comments
/// are not in the syntax tree; the docstring and type annotations are
/// skipped because they do not change behavior.
pub(super) fn fingerprint(def: &StmtFunctionDef) -> u64 {
    let mut hasher = Fnv1a::default();
    def.is_async.hash(&mut hasher);
    for decorator in &def.decorator_list {
        ComparableExpr::from(decorator).hash(&mut hasher);
    }
    let parameters = &def.parameters;
    for parameter in parameters
        .posonlyargs
        .iter()
        .chain(&parameters.args)
        .chain(&parameters.kwonlyargs)
    {
        parameter.parameter.name.as_str().hash(&mut hasher);
        parameter
            .default()
            .map(ComparableExpr::from)
            .hash(&mut hasher);
    }
    // Where each kind of parameter ends, so `a, *b` and `a, b` differ.
    parameters.posonlyargs.len().hash(&mut hasher);
    parameters.args.len().hash(&mut hasher);
    parameters
        .vararg
        .as_ref()
        .map(|parameter| parameter.name.as_str())
        .hash(&mut hasher);
    parameters
        .kwarg
        .as_ref()
        .map(|parameter| parameter.name.as_str())
        .hash(&mut hasher);
    let body = match def.body.split_first() {
        Some((Stmt::Expr(first), rest)) if matches!(*first.value, Expr::StringLiteral(_)) => rest,
        _ => &def.body[..],
    };
    for stmt in body {
        ComparableStmt::from(stmt).hash(&mut hasher);
    }
    hasher.finish()
}

/// FNV-1a, 64 bits: a small hash with a fixed algorithm, unlike the
/// standard library's default hasher, which may change between releases.
struct Fnv1a(u64);

impl Default for Fnv1a {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Hasher for Fnv1a {
    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

pub(super) fn is_staticmethod(def: &StmtFunctionDef) -> bool {
    def.decorator_list
        .iter()
        .any(|decorator| match &decorator.expression {
            Expr::Name(name) => name.id.as_str() == "staticmethod",
            _ => false,
        })
}

struct SuperFinder {
    found: bool,
}

impl<'a> Visitor<'a> for SuperFinder {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        if self.found || matches!(stmt, Stmt::FunctionDef(_) | Stmt::ClassDef(_)) {
            return;
        }
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        if self.found {
            return;
        }
        if let Expr::Call(call) = expr
            && matches!(&*call.func, Expr::Name(name) if name.id.as_str() == "super")
        {
            self.found = true;
            return;
        }
        walk_expr(self, expr);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ruff_python_parser::parse_module;

    fn method(lines: &[&str]) -> StmtFunctionDef {
        let parsed = parse_module(&lines.join("\n")).expect("test source must be valid Python");
        match parsed.into_syntax().body.into_iter().next() {
            Some(Stmt::FunctionDef(def)) => def,
            other => panic!("expected a function, got {other:?}"),
        }
    }

    #[test]
    fn reads_field_paths_from_api_decorators() {
        let def = method(&[
            "@api.depends('order_id.partner_id', 'amount', *EXTRA)",
            "@api.onchange('partner_id')",
            "@odoo.api.constrains('date_start', 'date_end')",
            "@api.depends_context('company')",
            "@other.depends('ignored')",
            "def _compute(self): pass",
        ]);
        let found: Vec<(ApiDecorator, Vec<String>)> = field_paths(&def)
            .into_iter()
            .map(|(kind, paths, _)| (kind, paths))
            .collect();
        let strings = |values: &[&str]| values.iter().map(|v| v.to_string()).collect::<Vec<_>>();
        assert_eq!(
            found,
            [
                (
                    ApiDecorator::Depends,
                    strings(&["order_id.partner_id", "amount"])
                ),
                (ApiDecorator::Onchange, strings(&["partner_id"])),
                (
                    ApiDecorator::Constrains,
                    strings(&["date_start", "date_end"])
                ),
            ]
        );
    }

    #[test]
    fn params_skip_the_receiver_and_keep_stars() {
        let def = method(&["def write(self, vals, /, *args, check=True, **kwargs): pass"]);
        assert_eq!(params(&def), ["vals", "*args", "check", "**kwargs"]);
    }

    #[test]
    fn classmethod_skips_cls_and_staticmethod_keeps_everything() {
        let class_method = method(&["@classmethod", "def _build(cls, env): pass"]);
        assert_eq!(params(&class_method), ["env"]);
        let static_method = method(&["@staticmethod", "def _clean(value, strict): pass"]);
        assert_eq!(params(&static_method), ["value", "strict"]);
    }

    #[test]
    fn finds_both_forms_of_super() {
        let bare = method(&[
            "def create(self, vals_list):",
            "    return super().create(vals_list)",
        ]);
        assert!(calls_super(&bare));
        let explicit = method(&[
            "def write(self, vals):",
            "    res = super(SaleOrder, self).write(vals)",
            "    return res",
        ]);
        assert!(calls_super(&explicit));
    }

    #[test]
    fn finds_super_inside_control_flow() {
        let def = method(&[
            "def unlink(self):",
            "    for record in self:",
            "        if record.state == 'draft':",
            "            return super().unlink()",
        ]);
        assert!(calls_super(&def));
    }

    #[test]
    fn ignores_super_in_nested_functions_and_missing_super() {
        let nested = method(&[
            "def _compute(self):",
            "    def helper():",
            "        return super().thing()",
            "    return helper",
        ]);
        assert!(!calls_super(&nested));
        let plain = method(&[
            "def name_get(self):",
            "    return [(r.id, r.name) for r in self]",
        ]);
        assert!(!calls_super(&plain));
    }

    fn fingerprint_of(lines: &[&str]) -> u64 {
        fingerprint(&method(lines))
    }

    #[test]
    fn fingerprint_ignores_what_does_not_change_behavior() {
        let plain = fingerprint_of(&[
            "@api.depends('order_line')",
            "def _compute_total(self):",
            "    for order in self:",
            "        order.total = sum(order.order_line.mapped('price'))",
        ]);
        let reformatted = fingerprint_of(&[
            "@api.depends(\"order_line\")",
            "def _compute_total(self) -> None:",
            "    \"\"\"Sum the lines.\"\"\"",
            "    # one pass over the orders",
            "    for order in self:",
            "        order.total = sum(",
            "            order.order_line.mapped(\"price\")",
            "        )",
        ]);
        assert_eq!(plain, reformatted);
    }

    #[test]
    fn fingerprint_changes_with_body_decorators_and_defaults() {
        let base = fingerprint_of(&[
            "@api.model",
            "def run(self, check=True):",
            "    return check",
        ]);
        for changed in [
            fingerprint_of(&[
                "@api.model",
                "def run(self, check=True):",
                "    return not check",
            ]),
            fingerprint_of(&["def run(self, check=True):", "    return check"]),
            fingerprint_of(&[
                "@api.model",
                "def run(self, check=False):",
                "    return check",
            ]),
            fingerprint_of(&["@api.model", "def run(self, *check):", "    return check"]),
        ] {
            assert_ne!(base, changed);
        }
    }
}
