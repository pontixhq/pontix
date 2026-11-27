//! What runs when a Python file is imported: its module-level imports and
//! the names it binds.

use pontix_core::Location;
use ruff_python_ast::visitor::{Visitor, walk_stmt};
use ruff_python_ast::{Expr, Stmt, StmtImport, StmtImportFrom};
use ruff_text_size::Ranged;

use crate::source::SourceFile;

/// One import, as written. `import a.b, c` is two imports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    /// Number of leading dots: `0` for an absolute import, `1` for `from . import x`.
    pub level: u32,
    /// The dotted module: `models` in `from .models import x`, `a.b` in
    /// `import a.b`. `None` for `from . import x`.
    pub module: Option<String>,
    /// The names after `import` in a `from` import, `*` included; empty for
    /// a plain `import a.b`.
    pub names: Vec<String>,
    /// Where the statement is.
    pub location: Location,
    /// The whole statement, exactly as written.
    pub statement: String,
}

#[derive(Debug, Default)]
pub(super) struct ModuleLevel {
    pub(super) imports: Vec<Import>,
    pub(super) bound_names: Vec<String>,
}

/// Walks the statements that run on import: those at module level,
/// including inside `if`, loops, `with` and the body, `else` and `finally`
/// of a `try`. Function and class bodies only run when called, and an
/// `except` handler only when the `try` failed, so their imports are left
/// out: `except ImportError:` fallbacks would otherwise look like broken
/// imports.
pub(super) fn module_level(source: &SourceFile<'_>, body: &[Stmt]) -> ModuleLevel {
    let mut collector = Collector {
        source,
        out: ModuleLevel::default(),
    };
    collector.visit_body(body);
    collector.out
}

struct Collector<'s, 'a> {
    source: &'s SourceFile<'a>,
    out: ModuleLevel,
}

impl Collector<'_, '_> {
    fn import(&mut self, stmt: &StmtImport) {
        for alias in &stmt.names {
            let module = alias.name.as_str();
            let bound = match &alias.asname {
                Some(asname) => asname.as_str(),
                None => module.split('.').next().unwrap_or(module),
            };
            self.out.bound_names.push(bound.to_string());
            self.out.imports.push(Import {
                level: 0,
                module: Some(module.to_string()),
                names: Vec::new(),
                location: self.source.location(stmt.start()),
                statement: self.source.text(stmt.range()).to_string(),
            });
        }
    }

    fn import_from(&mut self, stmt: &StmtImportFrom) {
        for alias in &stmt.names {
            let bound = alias.asname.as_ref().unwrap_or(&alias.name);
            if bound.as_str() != "*" {
                self.out.bound_names.push(bound.as_str().to_string());
            }
        }
        self.out.imports.push(Import {
            level: stmt.level,
            module: stmt
                .module
                .as_ref()
                .map(|module| module.as_str().to_string()),
            names: stmt
                .names
                .iter()
                .map(|alias| alias.name.as_str().to_string())
                .collect(),
            location: self.source.location(stmt.start()),
            statement: self.source.text(stmt.range()).to_string(),
        });
    }

    fn bind(&mut self, target: &Expr) {
        match target {
            Expr::Name(name) => self.out.bound_names.push(name.id.as_str().to_string()),
            Expr::Tuple(tuple) => tuple.elts.iter().for_each(|item| self.bind(item)),
            Expr::List(list) => list.elts.iter().for_each(|item| self.bind(item)),
            Expr::Starred(starred) => self.bind(&starred.value),
            _ => {}
        }
    }
}

impl<'a> Visitor<'a> for Collector<'_, '_> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::Import(import) => self.import(import),
            Stmt::ImportFrom(import) => self.import_from(import),
            Stmt::FunctionDef(function) => {
                self.out
                    .bound_names
                    .push(function.name.as_str().to_string());
            }
            Stmt::ClassDef(class) => self.out.bound_names.push(class.name.as_str().to_string()),
            Stmt::Assign(assign) => assign.targets.iter().for_each(|target| self.bind(target)),
            Stmt::AnnAssign(assign) if assign.value.is_some() => self.bind(&assign.target),
            Stmt::Try(try_stmt) => {
                self.visit_body(&try_stmt.body);
                self.visit_body(&try_stmt.orelse);
                self.visit_body(&try_stmt.finalbody);
            }
            other => walk_stmt(self, other),
        }
    }
}
