//! The Odoo framework's own Python files (`odoo/tools`, `odoo/orm`, ...):
//! the names each module binds, the methods of its classes, and which of
//! them core marks as deprecated.
//!
//! Core marks a deprecation in two ways, both read here: a
//! `warnings.warn(..., DeprecationWarning)` that runs every time the
//! function does (a statement of its body, not one inside a condition), or
//! a docstring that starts with "Deprecated".

use std::path::Path;

use pontix_core::Location;
use ruff_python_ast::{Expr, Stmt, StmtClassDef, StmtFunctionDef};
use ruff_python_parser::parse_module;
use ruff_text_size::Ranged;

use crate::source::SourceFile;

/// What one framework module defines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FrameworkFile {
    /// Names bound at module level, in source order.
    pub bindings: Vec<Binding>,
    /// Modules imported with `from module import *`.
    pub stars: Vec<String>,
    /// A module-level `__getattr__`: other names may exist at run time.
    pub dynamic: bool,
    /// A `DeprecationWarning` raised when the module is imported.
    pub deprecated: Option<Location>,
    /// Classes defined at module level, with their members.
    pub classes: Vec<FrameworkClass>,
    /// Whether the file could be parsed at all.
    pub parsed: bool,
}

/// A name a module binds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// The name bound.
    pub name: String,
    /// What binds it.
    pub kind: BindingKind,
    /// Where.
    pub location: Location,
}

/// How a module binds a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingKind {
    /// A function, a class or an assignment, with the deprecation core
    /// marks on it.
    Def {
        /// Where core marks it deprecated.
        deprecated: Option<Location>,
    },
    /// `from module import name`, under the name bound here.
    From {
        /// The module imported from, in full.
        module: String,
        /// The name in that module.
        name: String,
    },
    /// `import a.b as c` binds `c` to `a.b`; `import a.b` binds `a` to `a`.
    Module {
        /// The module, in full.
        module: String,
    },
}

/// A class and the names its body binds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameworkClass {
    /// The class name.
    pub name: String,
    /// The names of its bases as last written: `fields.Field` and
    /// `Field[int]` give `Field`. A base made by a call, such as a
    /// metaclass's dummy class, is left out.
    pub bases: Vec<String>,
    /// Its methods and attributes, in source order.
    pub members: Vec<Member>,
    /// The parameters its own `__init__` takes by name, without `*args`
    /// and `**kwargs`.
    pub init_params: Vec<String>,
}

/// A method or attribute of a framework class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// The member's name.
    pub name: String,
    /// Where it is defined.
    pub location: Location,
    /// Where core marks it deprecated.
    pub deprecated: Option<Location>,
}

/// Reads a framework file. `dotted` is its module name, such as
/// `odoo.tools.cache`; `package` is true for an `__init__.py`.
pub fn parse_framework(file: &Path, source: &str, dotted: &str, package: bool) -> FrameworkFile {
    let mut out = FrameworkFile::default();
    let Ok(parsed) = parse_module(source) else {
        return out;
    };
    out.parsed = true;
    let reader = Reader {
        source: SourceFile::new(file, source),
        package: if package {
            dotted.to_string()
        } else {
            dotted
                .rsplit_once('.')
                .map_or_else(String::new, |(parent, _)| parent.to_string())
        },
    };
    reader.body(&parsed.syntax().body, &mut out);
    out
}

struct Reader<'a> {
    source: SourceFile<'a>,
    /// The package relative imports start from.
    package: String,
}

impl Reader<'_> {
    fn body(&self, body: &[Stmt], out: &mut FrameworkFile) {
        for stmt in body {
            match stmt {
                Stmt::FunctionDef(def) => {
                    if def.name.as_str() == "__getattr__" {
                        out.dynamic = true;
                    }
                    self.def(def.name.as_str(), self.function(def), stmt, out);
                }
                Stmt::ClassDef(class) => {
                    let (members, deprecated) = self.class(class);
                    out.classes.push(FrameworkClass {
                        name: class.name.as_str().to_string(),
                        bases: class
                            .arguments
                            .as_deref()
                            .map(|arguments| arguments.args.iter().filter_map(base_name).collect())
                            .unwrap_or_default(),
                        members,
                        init_params: class
                            .body
                            .iter()
                            .find_map(|stmt| match stmt {
                                Stmt::FunctionDef(def) if def.name.as_str() == "__init__" => {
                                    Some(super::method::params(def))
                                }
                                _ => None,
                            })
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|param| !param.starts_with('*'))
                            .collect(),
                    });
                    self.def(class.name.as_str(), deprecated, stmt, out);
                }
                Stmt::Assign(assign) => {
                    for target in &assign.targets {
                        for name in names(target) {
                            self.def(&name, None, stmt, out);
                        }
                    }
                }
                Stmt::AnnAssign(assign) => {
                    for name in names(&assign.target) {
                        self.def(&name, None, stmt, out);
                    }
                }
                Stmt::Import(import) => {
                    for alias in &import.names {
                        let module = alias.name.as_str();
                        let (name, bound) = match &alias.asname {
                            Some(asname) => (asname.as_str().to_string(), module.to_string()),
                            None => {
                                let first = module.split('.').next().unwrap_or(module);
                                (first.to_string(), first.to_string())
                            }
                        };
                        out.bindings.push(Binding {
                            name,
                            kind: BindingKind::Module { module: bound },
                            location: self.source.location(stmt.start()),
                        });
                    }
                }
                Stmt::ImportFrom(import) => {
                    let Some(module) =
                        self.absolute(import.level, import.module.as_ref().map(|m| m.as_str()))
                    else {
                        continue;
                    };
                    for alias in &import.names {
                        let name = alias.name.as_str();
                        if name == "*" {
                            out.stars.push(module.clone());
                            continue;
                        }
                        let bound = alias.asname.as_ref().map_or(name, |asname| asname.as_str());
                        out.bindings.push(Binding {
                            name: bound.to_string(),
                            kind: BindingKind::From {
                                module: module.clone(),
                                name: name.to_string(),
                            },
                            location: self.source.location(stmt.start()),
                        });
                    }
                }
                Stmt::Expr(expr) if out.deprecated.is_none() && warns_deprecation(&expr.value) => {
                    out.deprecated = Some(self.source.location(stmt.start()));
                }
                // Names bound under a condition, such as an optional
                // dependency, may exist: they count as bound.
                Stmt::If(if_stmt) => {
                    self.nested(&if_stmt.body, out);
                    for clause in &if_stmt.elif_else_clauses {
                        self.nested(&clause.body, out);
                    }
                }
                Stmt::Try(try_stmt) => {
                    self.nested(&try_stmt.body, out);
                    for handler in &try_stmt.handlers {
                        let ruff_python_ast::ExceptHandler::ExceptHandler(handler) = handler;
                        self.nested(&handler.body, out);
                    }
                    self.nested(&try_stmt.orelse, out);
                    self.nested(&try_stmt.finalbody, out);
                }
                _ => {}
            }
        }
    }

    /// A conditional block: its bindings count, its warnings do not.
    fn nested(&self, body: &[Stmt], out: &mut FrameworkFile) {
        let deprecated = out.deprecated.clone();
        self.body(body, out);
        out.deprecated = deprecated;
    }

    fn def(&self, name: &str, deprecated: Option<Location>, stmt: &Stmt, out: &mut FrameworkFile) {
        out.bindings.push(Binding {
            name: name.to_string(),
            kind: BindingKind::Def { deprecated },
            location: self.source.location(stmt.start()),
        });
    }

    /// Where core marks the function deprecated, if it does.
    fn function(&self, def: &StmtFunctionDef) -> Option<Location> {
        let docstring = def.body.first().and_then(|first| match first {
            Stmt::Expr(expr) => match &*expr.value {
                Expr::StringLiteral(literal) => Some(literal.value.to_str()),
                _ => None,
            },
            _ => None,
        });
        if docstring.is_some_and(|text| {
            text.trim_start()
                .get(..10)
                .is_some_and(|start| start.eq_ignore_ascii_case("deprecated"))
        }) {
            return Some(self.source.location(def.name.start()));
        }
        def.body.iter().find_map(|stmt| match stmt {
            Stmt::Expr(expr) if warns_deprecation(&expr.value) => {
                Some(self.source.location(stmt.start()))
            }
            _ => None,
        })
    }

    /// The class's members, and where it is deprecated: its `__init__` or
    /// `__new__` warning every time.
    fn class(&self, class: &StmtClassDef) -> (Vec<Member>, Option<Location>) {
        let mut members = Vec::new();
        let mut deprecated = None;
        for stmt in &class.body {
            match stmt {
                Stmt::FunctionDef(def) => {
                    let warned = self.function(def);
                    if matches!(def.name.as_str(), "__init__" | "__new__") && deprecated.is_none() {
                        deprecated.clone_from(&warned);
                    }
                    members.push(Member {
                        name: def.name.as_str().to_string(),
                        location: self.source.location(def.name.start()),
                        deprecated: warned,
                    });
                }
                Stmt::Assign(assign) => {
                    for target in &assign.targets {
                        for name in names(target) {
                            // `__slots__ = ['env', ...]` declares attributes.
                            if name == "__slots__" {
                                for slot in slots(&assign.value) {
                                    members.push(Member {
                                        name: slot,
                                        location: self.source.location(stmt.start()),
                                        deprecated: None,
                                    });
                                }
                            }
                            members.push(Member {
                                name,
                                location: self.source.location(stmt.start()),
                                deprecated: None,
                            });
                        }
                    }
                }
                Stmt::AnnAssign(assign) => {
                    for name in names(&assign.target) {
                        members.push(Member {
                            name,
                            location: self.source.location(stmt.start()),
                            deprecated: None,
                        });
                    }
                }
                _ => {}
            }
        }
        (members, deprecated)
    }

    /// The absolute name of the module an import names.
    fn absolute(&self, level: u32, module: Option<&str>) -> Option<String> {
        if level == 0 {
            return module.map(str::to_string);
        }
        let mut base = self.package.clone();
        for _ in 1..level {
            base = base.rsplit_once('.')?.0.to_string();
        }
        Some(match module {
            Some(module) => format!("{base}.{module}"),
            None => base,
        })
    }
}

/// `warnings.warn(..., DeprecationWarning)` or `warn(...)` with that
/// category, given as the second argument or as `category=`.
fn warns_deprecation(expr: &Expr) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    let named_warn = match &*call.func {
        Expr::Attribute(attribute) => attribute.attr.as_str() == "warn",
        Expr::Name(name) => name.id.as_str() == "warn",
        _ => false,
    };
    if !named_warn {
        return false;
    }
    let is_deprecation = |expr: &Expr| match expr {
        Expr::Name(name) => name.id.as_str() == "DeprecationWarning",
        Expr::Attribute(attribute) => attribute.attr.as_str() == "DeprecationWarning",
        _ => false,
    };
    call.arguments.args.get(1).is_some_and(is_deprecation)
        || call
            .arguments
            .find_keyword("category")
            .is_some_and(|keyword| is_deprecation(&keyword.value))
}

/// The names a `__slots__` literal lists.
fn slots(value: &Expr) -> Vec<String> {
    let elements: &[Expr] = match value {
        Expr::List(list) => &list.elts,
        Expr::Tuple(tuple) => &tuple.elts,
        _ => &[],
    };
    elements
        .iter()
        .filter_map(|element| match element {
            Expr::StringLiteral(literal) => Some(literal.value.to_str().to_string()),
            _ => None,
        })
        .collect()
}

fn names(target: &Expr) -> Vec<String> {
    match target {
        Expr::Name(name) => vec![name.id.as_str().to_string()],
        Expr::Tuple(tuple) => tuple.elts.iter().flat_map(names).collect(),
        Expr::List(list) => list.elts.iter().flat_map(names).collect(),
        _ => Vec::new(),
    }
}

/// The name a class base is written with: `Field`, `fields.Field` or
/// `Field[int]`.
fn base_name(base: &Expr) -> Option<String> {
    match base {
        Expr::Name(name) => Some(name.id.as_str().to_string()),
        Expr::Attribute(attribute) => Some(attribute.attr.as_str().to_string()),
        Expr::Subscript(subscript) => base_name(&subscript.value),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deprecated_names(file: &FrameworkFile) -> Vec<&str> {
        file.bindings
            .iter()
            .filter(|binding| {
                matches!(
                    binding.kind,
                    BindingKind::Def {
                        deprecated: Some(_)
                    }
                )
            })
            .map(|binding| binding.name.as_str())
            .collect()
    }

    #[test]
    fn reads_bindings_imports_and_deprecations() {
        let source = [
            "import warnings",
            "import odoo.orm.domains as domains",
            "from .cache import ormcache, ormcache_context as oc",
            "from ..misc import *",
            "try:",
            "    import babel",
            "except ImportError:",
            "    babel = None",
            "def AND(domains):",
            "    warnings.warn('Since 19.0, use Domain', DeprecationWarning)",
            "def maybe(x):",
            "    if x:",
            "        warnings.warn('conditional', DeprecationWarning)",
            "def old():",
            "    \"\"\"Deprecated - use new().\"\"\"",
            "class ormcache_context:",
            "    def __init__(self, *args, keys):",
            "        warnings.warn('use ormcache', category=DeprecationWarning)",
            "    def determine_key(self):",
            "        pass",
            "    __slots__ = ['env', '_ids']",
            "TRUE_LEAF = (1, '=', 1)",
        ]
        .join("\n");
        let file = parse_framework(Path::new("odoo/tools/x.py"), &source, "odoo.tools.x", false);
        assert!(file.parsed);
        assert_eq!(deprecated_names(&file), ["AND", "old", "ormcache_context"]);
        assert_eq!(file.stars, ["odoo.misc"]);
        let froms: Vec<(&str, &str, &str)> = file
            .bindings
            .iter()
            .filter_map(|binding| match &binding.kind {
                BindingKind::From { module, name } => {
                    Some((binding.name.as_str(), module.as_str(), name.as_str()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            froms,
            [
                ("ormcache", "odoo.tools.cache", "ormcache"),
                ("oc", "odoo.tools.cache", "ormcache_context"),
            ]
        );
        assert!(file.bindings.iter().any(|binding| binding.name == "domains"
            && binding.kind
                == BindingKind::Module {
                    module: "odoo.orm.domains".to_string()
                }));
        assert!(file.bindings.iter().any(|binding| binding.name == "babel"));
        let class = &file.classes[0];
        assert_eq!(class.members[1].name, "determine_key");
        let members: Vec<&str> = class.members.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(members[2..], ["env", "_ids", "__slots__"]);
        assert!(class.members[1].deprecated.is_none());
        assert!(file.deprecated.is_none());
        assert!(!file.dynamic);
    }

    #[test]
    fn a_module_level_warning_deprecates_the_module() {
        let source = "import warnings\nwarnings.warn('gone soon', DeprecationWarning)\n";
        let file = parse_framework(Path::new("odoo/osv/__init__.py"), source, "odoo.osv", true);
        assert!(file.deprecated.is_some());
        let source = "def __getattr__(name):\n    return name\n";
        assert!(parse_framework(Path::new("odoo/x.py"), source, "odoo.x", false).dynamic);
    }
}
