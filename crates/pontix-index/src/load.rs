//! Following imports from a module's `__init__.py`, the way Python runs them.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use pontix_core::{Diagnostic, DiagnosticKind, ModelDef, ModuleName, XmlRef};
use pontix_parsers::{Candidate, Import, parse_python};

use crate::files::Root;

/// The Python side of one module, in the order Odoo loads it.
#[derive(Debug, Default)]
pub(crate) struct Loaded {
    pub(crate) files: Vec<PathBuf>,
    /// Model classes and candidates, in the order Python defines them.
    pub(crate) classes: Vec<Class>,
    /// What the base resolution needs from each file, in `files` order.
    pub(crate) sources: Vec<PythonSource>,
    /// `env.ref(...)` calls in those files.
    pub(crate) xmlid_refs: Vec<XmlRef>,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

/// A class found in a module: a model, or a candidate whose bases are
/// resolved once every module is read.
#[derive(Debug)]
pub(crate) enum Class {
    Model(ModelDef),
    Candidate {
        candidate: Candidate,
        /// Index of its file in [`Loaded::sources`].
        source: usize,
    },
}

/// A loaded Python file as the rest of the Python world sees it.
#[derive(Debug)]
pub(crate) struct PythonSource {
    /// Its dotted module name, such as `odoo.addons.base.models.res_users`.
    pub(crate) dotted: String,
    /// Whether it is a package's `__init__.py`.
    pub(crate) is_package: bool,
    pub(crate) imports: Vec<Import>,
    /// Class names of the models it defines itself.
    pub(crate) models: Vec<String>,
}

/// Loads every Python file of the module that Python would run when Odoo
/// imports `odoo.addons.<module>`.
pub(crate) fn load(root: &Root, module_dir: &Path, module: &ModuleName) -> Loaded {
    let mut loader = Loader {
        root,
        module_dir,
        module: module.as_str(),
        visited: HashSet::new(),
        bound_names: HashMap::new(),
        out: Loaded::default(),
    };
    loader.file(&module_dir.join("__init__.py"));
    loader.out
}

/// What one dotted name refers to.
enum Target {
    /// A package directory, with or without `__init__.py`.
    Package(PathBuf),
    /// A `.py` file; names imported from it are its attributes.
    Module,
}

impl Class {
    fn line(&self) -> std::num::NonZeroU32 {
        match self {
            Self::Model(model) => model.location.line,
            Self::Candidate { candidate, .. } => candidate.model.location.line,
        }
    }
}

struct Loader<'a> {
    root: &'a Root,
    module_dir: &'a Path,
    module: &'a str,
    visited: HashSet<PathBuf>,
    /// Module-level names of every file read, by canonical path.
    bound_names: HashMap<PathBuf, Vec<String>>,
    out: Loaded,
}

impl Loader<'_> {
    /// Runs one file: its models and the files it imports, interleaved in
    /// statement order, since an import loads the other file at that point.
    fn file(&mut self, path: &Path) {
        let canonical = match self.root.resolve(path) {
            Ok(Some(canonical)) => canonical,
            Ok(None) => return,
            Err(diagnostic) => return self.out.diagnostics.push(diagnostic),
        };
        if !self.visited.insert(canonical.clone()) {
            return;
        }
        let source = match self.root.read(path) {
            Ok(source) => source,
            Err(diagnostic) => return self.out.diagnostics.push(diagnostic),
        };
        let parsed = parse_python(path, &source);
        self.bound_names.insert(canonical, parsed.bound_names);
        self.out.files.push(path.to_path_buf());
        self.out.diagnostics.extend(parsed.diagnostics);
        self.out.xmlid_refs.extend(parsed.xmlid_refs);

        let index = self.out.sources.len();
        self.out.sources.push(PythonSource {
            dotted: self.dotted(path),
            is_package: path.file_name().is_some_and(|name| name == "__init__.py"),
            imports: parsed.imports.clone(),
            models: parsed.models.iter().map(|m| m.class_name.clone()).collect(),
        });

        let mut classes: Vec<Class> = parsed
            .models
            .into_iter()
            .map(Class::Model)
            .chain(
                parsed
                    .candidates
                    .into_iter()
                    .map(|candidate| Class::Candidate {
                        candidate,
                        source: index,
                    }),
            )
            .collect();
        classes.sort_by_key(Class::line);
        let mut classes = classes.into_iter().peekable();
        for import in &parsed.imports {
            while let Some(class) = classes.next_if(|class| class.line() < import.location.line) {
                self.out.classes.push(class);
            }
            self.import(path, import);
        }
        self.out.classes.extend(classes);
    }

    /// `odoo.addons.<module>` followed by the file's path inside the module:
    /// `models/sale.py` is `odoo.addons.<module>.models.sale` and
    /// `models/__init__.py` is `odoo.addons.<module>.models`.
    fn dotted(&self, path: &Path) -> String {
        let mut dotted = format!("odoo.addons.{}", self.module);
        let Ok(relative) = path.strip_prefix(self.module_dir) else {
            return dotted;
        };
        for component in relative.with_extension("").components() {
            let part = component.as_os_str().to_string_lossy();
            if part != "__init__" {
                dotted.push('.');
                dotted.push_str(&part);
            }
        }
        dotted
    }

    fn import(&mut self, current: &Path, import: &Import) {
        let Some((base, segments)) = self.base(current, import) else {
            return;
        };
        let mut target = Target::Package(base);
        for segment in segments {
            // `a.b` only resolves when `a` is a package.
            let Target::Package(dir) = target else {
                return self.unresolved(import);
            };
            target = match self.step(&dir, segment) {
                Some(next) => next,
                None => return self.unresolved(import),
            };
        }
        let Target::Package(dir) = target else {
            return;
        };
        let mut missing = false;
        for name in import.names.iter().filter(|name| name.as_str() != "*") {
            // A name that is no file may be bound in the package's
            // `__init__.py`. `from .models import SaleOrder` is never checked:
            // `SaleOrder` is usually a class, not a file.
            if self.step(&dir, name).is_none() && import.module.is_none() && !self.binds(&dir, name)
            {
                missing = true;
            }
        }
        if missing {
            self.unresolved(import);
        }
    }

    /// The package an import starts from and the dotted segments below it.
    /// `None` for imports outside this module, which load other modules.
    fn base<'i>(&self, current: &Path, import: &'i Import) -> Option<(PathBuf, Vec<&'i str>)> {
        let segments: Vec<&str> = import
            .module
            .as_deref()
            .map(|module| module.split('.').collect())
            .unwrap_or_default();
        if import.level == 0 {
            // Absolute imports load this module's files only when they name
            // it: `odoo.addons.<module>...`.
            return match segments.as_slice() {
                ["odoo", "addons", name, rest @ ..] if *name == self.module => {
                    Some((self.module_dir.to_path_buf(), rest.to_vec()))
                }
                _ => None,
            };
        }
        let mut dir = current.parent()?;
        for _ in 1..import.level {
            dir = dir.parent()?;
        }
        dir.starts_with(self.module_dir)
            .then(|| (dir.to_path_buf(), segments))
    }

    /// Imports `dir.name` the way Python's path finder does: a regular
    /// package (`name/__init__.py`) first, then a module (`name.py`), then a
    /// namespace package (a plain `name/` directory).
    fn step(&mut self, dir: &Path, name: &str) -> Option<Target> {
        let package = dir.join(name);
        let init = package.join("__init__.py");
        if init.is_file() {
            self.file(&init);
            return Some(Target::Package(package));
        }
        let module = dir.join(format!("{name}.py"));
        if module.is_file() {
            self.file(&module);
            return Some(Target::Module);
        }
        package.is_dir().then_some(Target::Package(package))
    }

    /// Whether the package's `__init__.py` binds `name` at module level.
    fn binds(&self, package: &Path, name: &str) -> bool {
        let Ok(Some(init)) = self.root.resolve(&package.join("__init__.py")) else {
            return false;
        };
        self.bound_names
            .get(&init)
            .is_some_and(|names| names.iter().any(|bound| bound == name))
    }

    fn unresolved(&mut self, import: &Import) {
        self.out.diagnostics.push(Diagnostic {
            kind: DiagnosticKind::UnresolvedImport,
            location: import.location.clone(),
            snippet: import.statement.clone(),
        });
    }
}
