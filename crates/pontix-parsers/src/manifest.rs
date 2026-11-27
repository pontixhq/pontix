//! Module manifests: `__manifest__.py`, read the way Odoo reads it.
//!
//! Odoo evaluates the file with `ast.literal_eval`: the whole file must be one
//! literal expression, and if it is not, Odoo silently ignores the module.
//! Pontix applies the same rule and reports the first part that is not a
//! literal, because a module Odoo cannot see is a finding in itself.

use std::path::Path;

use pontix_core::{AutoInstall, Diagnostic, DiagnosticKind, Manifest, ModuleName};
use ruff_python_ast::{Expr, ExprList, ExprSet, ExprTuple, Operator, UnaryOp};
use ruff_python_parser::parse_expression;
use ruff_text_size::{Ranged, TextRange};

use crate::source::SourceFile;

/// What one `__manifest__.py` declares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManifestFile {
    /// The manifest, or `None` when Odoo could not read it either.
    pub manifest: Option<Manifest>,
    /// Everything that could not be read, in source order.
    pub diagnostics: Vec<Diagnostic>,
}

/// Reads one module manifest. `file` is only recorded in locations.
pub fn parse_manifest(file: &Path, source: &str) -> ManifestFile {
    let source_file = SourceFile::new(file, source);
    let mut out = ManifestFile::default();
    match parse_expression(source) {
        Ok(parsed) => {
            let expr = &*parsed.syntax().body;
            match (first_non_literal(expr), expr) {
                (Some(part), _) => out.diagnostics.push(
                    source_file.diagnostic(DiagnosticKind::UnrecognizedManifest, part.range()),
                ),
                (None, Expr::Dict(dict)) => {
                    let mut manifest = Manifest::default();
                    for item in &dict.items {
                        // A literal dict has no `**` entries, so every key is present.
                        let Some(key) = item.key.as_ref() else {
                            continue;
                        };
                        if read_key(&mut manifest, key, &item.value).is_none() {
                            let range = TextRange::new(key.start(), item.value.end());
                            out.diagnostics.push(
                                source_file
                                    .diagnostic(DiagnosticKind::UnrecognizedManifestValue, range),
                            );
                        }
                    }
                    out.manifest = Some(manifest);
                }
                (None, other) => out.diagnostics.push(
                    source_file.diagnostic(DiagnosticKind::UnrecognizedManifest, other.range()),
                ),
            }
        }
        Err(error) => out.diagnostics.push(
            source_file
                .line_diagnostic(DiagnosticKind::UnrecognizedManifest, error.location.start()),
        ),
    }
    out
}

/// Stores one key of the manifest. `None` means a key Pontix reads holds a
/// value of the wrong shape; the key is then left as if it were not written.
/// Keys Pontix does not read are accepted as they are.
fn read_key(manifest: &mut Manifest, key: &Expr, value: &Expr) -> Option<()> {
    let Expr::StringLiteral(key) = key else {
        return Some(());
    };
    match key.value.to_str() {
        "name" => manifest.name = Some(string(value)?.to_string()),
        "version" => manifest.version = Some(string(value)?.to_string()),
        "depends" => manifest.depends = strings(value)?.map(ModuleName::new).collect(),
        "data" => manifest.data = strings(value)?.map(str::to_string).collect(),
        "installable" => manifest.installable = Some(boolean(value)?),
        "auto_install" => manifest.auto_install = auto_install(value)?,
        "external_dependencies" => manifest.external_python = external_python(value)?,
        _ => {}
    }
    Some(())
}

fn string(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::StringLiteral(literal) => Some(literal.value.to_str()),
        _ => None,
    }
}

fn boolean(expr: &Expr) -> Option<bool> {
    match expr {
        Expr::BooleanLiteral(literal) => Some(literal.value),
        _ => None,
    }
}

/// A list or tuple of strings, in order.
fn strings(expr: &Expr) -> Option<impl Iterator<Item = &str>> {
    let (Expr::List(ExprList { elts, .. }) | Expr::Tuple(ExprTuple { elts, .. })) = expr else {
        return None;
    };
    let items: Vec<&str> = elts.iter().map(string).collect::<Option<_>>()?;
    Some(items.into_iter())
}

fn auto_install(expr: &Expr) -> Option<AutoInstall> {
    match expr {
        Expr::BooleanLiteral(literal) if literal.value => Some(AutoInstall::WithAllDepends),
        Expr::BooleanLiteral(_) => Some(AutoInstall::No),
        other => Some(AutoInstall::When(
            strings(other)?.map(ModuleName::new).collect(),
        )),
    }
}

/// The `python` entry of `external_dependencies`; other entries (`bin`,
/// `apt`) are not read yet.
fn external_python(expr: &Expr) -> Option<Vec<String>> {
    let Expr::Dict(dict) = expr else {
        return None;
    };
    let python = dict
        .items
        .iter()
        .find(|item| item.key.as_ref().and_then(string) == Some("python"));
    match python {
        Some(item) => Some(strings(&item.value)?.map(str::to_string).collect()),
        None => Some(Vec::new()),
    }
}

/// The first part of `expr` that `ast.literal_eval` would reject, or `None`
/// when the whole expression is a literal: strings, bytes, numbers, booleans,
/// `None`, `...`, tuples, lists, sets, dicts, `set()`, a signed number, and
/// the sum or difference of two numbers (complex numbers).
fn first_non_literal(expr: &Expr) -> Option<&Expr> {
    match expr {
        Expr::StringLiteral(_)
        | Expr::BytesLiteral(_)
        | Expr::NumberLiteral(_)
        | Expr::BooleanLiteral(_)
        | Expr::NoneLiteral(_)
        | Expr::EllipsisLiteral(_) => None,
        Expr::List(ExprList { elts, .. })
        | Expr::Tuple(ExprTuple { elts, .. })
        | Expr::Set(ExprSet { elts, .. }) => elts.iter().find_map(first_non_literal),
        Expr::Dict(dict) => dict.items.iter().find_map(|item| match &item.key {
            Some(key) => first_non_literal(key).or_else(|| first_non_literal(&item.value)),
            None => Some(&item.value),
        }),
        Expr::Call(call)
            if matches!(&*call.func, Expr::Name(name) if name.id.as_str() == "set")
                && call.arguments.args.is_empty()
                && call.arguments.keywords.is_empty() =>
        {
            None
        }
        Expr::UnaryOp(unary)
            if matches!(unary.op, UnaryOp::UAdd | UnaryOp::USub) && is_number(&unary.operand) =>
        {
            None
        }
        Expr::BinOp(binary)
            if matches!(binary.op, Operator::Add | Operator::Sub)
                && is_signed_number(&binary.left)
                && is_number(&binary.right) =>
        {
            None
        }
        other => Some(other),
    }
}

fn is_number(expr: &Expr) -> bool {
    matches!(expr, Expr::NumberLiteral(_))
}

fn is_signed_number(expr: &Expr) -> bool {
    match expr {
        Expr::UnaryOp(unary) => {
            matches!(unary.op, UnaryOp::UAdd | UnaryOp::USub) && is_number(&unary.operand)
        }
        other => is_number(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "sale_custom/__manifest__.py";

    fn parse(lines: &[&str]) -> ManifestFile {
        parse_manifest(Path::new(FILE), &lines.join("\n"))
    }

    fn modules(names: &[&str]) -> Vec<ModuleName> {
        names.iter().copied().map(ModuleName::new).collect()
    }

    fn kinds(file: &ManifestFile) -> Vec<(DiagnosticKind, String, &str)> {
        file.diagnostics
            .iter()
            .map(|d| (d.kind, d.location.to_string(), d.snippet.as_str()))
            .collect()
    }

    #[test]
    fn reads_every_key_pontix_uses() {
        let file = parse(&[
            "# Copyright 2026 Example",
            "{",
            "    'name': 'Sale Custom',",
            "    'version': '19.0.1.0.0',",
            "    'license': 'AGPL-3',",
            "    'depends': ['sale', 'stock'],",
            "    'data': ['security/ir.model.access.csv', 'views/sale_order.xml'],",
            "    'installable': True,",
            "    'auto_install': ['sale'],",
            "    'external_dependencies': {'python': ['openupgradelib'], 'bin': ['wkhtmltopdf']},",
            "}",
        ]);
        assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
        assert_eq!(
            file.manifest,
            Some(Manifest {
                name: Some("Sale Custom".to_string()),
                version: Some("19.0.1.0.0".to_string()),
                depends: modules(&["sale", "stock"]),
                data: vec![
                    "security/ir.model.access.csv".to_string(),
                    "views/sale_order.xml".to_string(),
                ],
                installable: Some(true),
                auto_install: AutoInstall::When(modules(&["sale"])),
                external_python: vec!["openupgradelib".to_string()],
            })
        );
    }

    #[test]
    fn missing_keys_stay_as_not_written() {
        let file = parse(&["{'name': 'Minimal'}"]);
        let manifest = file.manifest.expect("a literal dict is a manifest");
        assert_eq!(manifest.version, None);
        assert!(manifest.depends.is_empty());
        assert_eq!(manifest.installable, None);
        assert_eq!(manifest.auto_install, AutoInstall::No);
    }

    #[test]
    fn auto_install_true_and_tuples_are_read() {
        let file = parse(&["{'depends': ('sale', 'stock'), 'auto_install': True}"]);
        let manifest = file.manifest.expect("a literal dict is a manifest");
        assert_eq!(manifest.depends, modules(&["sale", "stock"]));
        assert_eq!(manifest.auto_install, AutoInstall::WithAllDepends);
    }

    #[test]
    fn a_non_literal_anywhere_hides_the_whole_module() {
        let file = parse(&[
            "{",
            "    'name': 'Sale Custom',",
            "    'version': VERSION,",
            "}",
        ]);
        assert_eq!(file.manifest, None);
        assert_eq!(
            kinds(&file),
            [(
                DiagnosticKind::UnrecognizedManifest,
                format!("{FILE}:3"),
                "VERSION"
            )]
        );
    }

    #[test]
    fn a_docstring_before_the_dict_is_not_one_expression() {
        let file = parse(&["\"\"\"Sale Custom.\"\"\"", "{'name': 'Sale Custom'}"]);
        assert_eq!(file.manifest, None);
        assert_eq!(file.diagnostics.len(), 1);
        assert_eq!(
            file.diagnostics[0].kind,
            DiagnosticKind::UnrecognizedManifest
        );
    }

    #[test]
    fn a_literal_that_is_not_a_dict_is_reported() {
        let file = parse(&["['sale']"]);
        assert_eq!(file.manifest, None);
        assert_eq!(
            kinds(&file),
            [(
                DiagnosticKind::UnrecognizedManifest,
                format!("{FILE}:1"),
                "['sale']"
            )]
        );
    }

    #[test]
    fn wrong_shapes_are_reported_and_left_unread() {
        let file = parse(&[
            "{",
            "    'name': 'Sale Custom',",
            "    'depends': 'sale',",
            "    'installable': 1,",
            "}",
        ]);
        let manifest = file.manifest.as_ref().expect("still a literal dict");
        assert_eq!(manifest.name.as_deref(), Some("Sale Custom"));
        assert!(manifest.depends.is_empty());
        assert_eq!(manifest.installable, None);
        assert_eq!(
            kinds(&file),
            [
                (
                    DiagnosticKind::UnrecognizedManifestValue,
                    format!("{FILE}:3"),
                    "'depends': 'sale'"
                ),
                (
                    DiagnosticKind::UnrecognizedManifestValue,
                    format!("{FILE}:4"),
                    "'installable': 1"
                ),
            ]
        );
    }

    #[test]
    fn literal_eval_accepts_what_python_accepts() {
        for source in [
            "{'x': -1, 'y': 1+2j, 'z': set(), 'w': {1, 2}, 'v': b'raw', 'u': None, 't': ...}",
            "{'name': 'Implicit' ' concatenation'}",
        ] {
            let file = parse(&[source]);
            assert!(file.manifest.is_some(), "{source}: {:?}", file.diagnostics);
        }
        for source in [
            "{'x': f'{1}'}",
            "{'x': [*BASE]}",
            "{**BASE}",
            "{'x': not True}",
        ] {
            assert_eq!(parse(&[source]).manifest, None, "{source}");
        }
    }

    #[test]
    fn invalid_python_is_reported_on_its_line() {
        let file = parse(&["{", "    'name': 'Sale Custom',,", "}"]);
        assert_eq!(file.manifest, None);
        assert_eq!(
            kinds(&file),
            [(
                DiagnosticKind::UnrecognizedManifest,
                format!("{FILE}:2"),
                "    'name': 'Sale Custom',,"
            )]
        );
    }
}
