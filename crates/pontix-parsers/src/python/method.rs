//! Method definitions in a model class body.

use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};
use ruff_python_ast::{Expr, Stmt, StmtFunctionDef};

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

/// True when the method body calls `super()` or `super(Class, self)`.
/// Functions and classes nested in the body are not counted: their `super()`
/// belongs to them, not to the method.
pub(super) fn calls_super(def: &StmtFunctionDef) -> bool {
    let mut finder = SuperFinder { found: false };
    finder.visit_body(&def.body);
    finder.found
}

fn is_staticmethod(def: &StmtFunctionDef) -> bool {
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
}
