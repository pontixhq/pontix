//! `odoo/release.py`: which Odoo version a source tree holds.

use ruff_python_ast::{Expr, Number, Stmt};
use ruff_python_parser::parse_module;

/// The first two numbers of `version_info = (19, 0, 0, FINAL, 0, '')`.
/// `None` when the file is not valid Python or does not assign
/// `version_info` a tuple starting with two integers, as for a SaaS build
/// (`('saas~18', 2, ...)`).
pub fn release_version(source: &str) -> Option<(u32, u32)> {
    let parsed = parse_module(source).ok()?;
    parsed.syntax().body.iter().rev().find_map(|stmt| {
        let Stmt::Assign(assign) = stmt else {
            return None;
        };
        let named = assign
            .targets
            .iter()
            .any(|target| matches!(target, Expr::Name(name) if name.id.as_str() == "version_info"));
        let Expr::Tuple(tuple) = &*assign.value else {
            return None;
        };
        if !named {
            return None;
        }
        let number = |index: usize| match tuple.elts.get(index)? {
            Expr::NumberLiteral(literal) => match &literal.value {
                Number::Int(value) => value.as_u64().and_then(|value| u32::try_from(value).ok()),
                _ => None,
            },
            _ => None,
        };
        Some((number(0)?, number(1)?))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_major_and_minor_version() {
        let source = "RELEASE_LEVELS = [ALPHA, BETA, RELEASE_CANDIDATE, FINAL] = ['alpha', 'beta', 'candidate', 'final']\n\
                      version_info = (19, 0, 0, FINAL, 0, '')\n\
                      series = serie = major_version = '.'.join(str(s) for s in version_info[:2])\n";
        assert_eq!(release_version(source), Some((19, 0)));
    }

    #[test]
    fn other_shapes_are_not_a_version() {
        assert_eq!(
            release_version("version_info = ('saas~18', 2, 0, FINAL, 0, '')"),
            None
        );
        assert_eq!(release_version("version = '19.0'"), None);
        assert_eq!(release_version("version_info = ("), None);
    }
}
