//! Classes whose model base lives in another file, such as
//! `class CheckIdentityPasskeys(CheckIdentity)` with `CheckIdentity` imported
//! from `odoo.addons.base.models.res_users` (`auth_passkey`, Odoo 18).
//!
//! Python makes a class a model when any base is a model class, wherever that
//! base was defined. Once every module is read, each candidate's bases are
//! followed through the imports of its file, including names re-exported by
//! other files, until nothing changes.

use std::collections::{HashMap, HashSet};

use pontix_core::{Diagnostic, ModelDef};
use pontix_parsers::Import;

use crate::load::{Class, PythonSource};

/// Turns candidates into models where a base resolves to a model class, and
/// reports the others that set model attributes. `classes[m]` are the classes
/// of module `m`, referring to `sources` by index.
pub(crate) fn resolve(
    classes: Vec<Vec<Class>>,
    sources: &[PythonSource],
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<Vec<ModelDef>> {
    let mut world = World::new(sources);
    let mut resolved: HashSet<(usize, usize)> = HashSet::new();
    loop {
        let mut changed = world.follow_reexports();
        for (module, module_classes) in classes.iter().enumerate() {
            for (index, class) in module_classes.iter().enumerate() {
                let Class::Candidate { candidate, source } = class else {
                    continue;
                };
                if resolved.contains(&(module, index)) {
                    continue;
                }
                let line = candidate.model.location.line;
                if candidate
                    .bases
                    .iter()
                    .any(|base| world.is_model(*source, base, line))
                {
                    resolved.insert((module, index));
                    world.define(*source, &candidate.model.class_name, line);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    classes
        .into_iter()
        .enumerate()
        .map(|(module, module_classes)| {
            module_classes
                .into_iter()
                .enumerate()
                .filter_map(|(index, class)| match class {
                    Class::Model(model) => Some(model),
                    Class::Candidate { candidate, .. } => {
                        if resolved.contains(&(module, index)) {
                            diagnostics.extend(candidate.diagnostics);
                            Some(candidate.model)
                        } else {
                            if candidate.sets_model_attributes {
                                diagnostics.push(candidate.unrecognized);
                            }
                            None
                        }
                    }
                })
                .collect()
        })
        .collect()
}

/// Which names are bound to model classes, file by file.
struct World<'s> {
    sources: &'s [PythonSource],
    by_dotted: HashMap<&'s str, usize>,
    /// Names other files can import from each file as model classes.
    exports: Vec<HashSet<String>>,
    /// Names bound to model classes by the file's own imports.
    imported: Vec<HashSet<String>>,
    /// Candidates resolved in each file, with the line they are defined on.
    defined: Vec<Vec<(String, std::num::NonZeroU32)>>,
}

impl<'s> World<'s> {
    fn new(sources: &'s [PythonSource]) -> Self {
        Self {
            sources,
            by_dotted: sources
                .iter()
                .enumerate()
                .map(|(index, source)| (source.dotted.as_str(), index))
                .collect(),
            exports: sources
                .iter()
                .map(|source| source.models.iter().cloned().collect())
                .collect(),
            imported: vec![HashSet::new(); sources.len()],
            defined: vec![Vec::new(); sources.len()],
        }
    }

    fn define(&mut self, source: usize, name: &str, line: std::num::NonZeroU32) {
        self.defined[source].push((name.to_string(), line));
        self.exports[source].insert(name.to_string());
    }

    /// `from m import X as Y` binds `Y` to a model class when `m` exports `X`.
    fn follow_reexports(&mut self) -> bool {
        let mut changed = false;
        for (index, source) in self.sources.iter().enumerate() {
            for import in &source.imports {
                if import.names.is_empty() {
                    continue;
                }
                let Some(target) = self
                    .module(index, import)
                    .and_then(|m| self.by_dotted.get(m.as_str()))
                else {
                    continue;
                };
                for (name, binding) in import.names.iter().zip(&import.bindings) {
                    if !binding.is_empty() && self.exports[*target].contains(name) {
                        let added = self.imported[index].insert(binding.clone());
                        let exported = self.exports[index].insert(binding.clone());
                        changed |= added || exported;
                    }
                }
            }
        }
        changed
    }

    /// Whether `base`, written in `source` on a class defined at `line`, is a
    /// model class there.
    fn is_model(&self, source: usize, base: &str, line: std::num::NonZeroU32) -> bool {
        let Some((first, rest)) = base.split_once('.') else {
            // A bare name: imported as a model, or a resolved candidate
            // defined above. Models defined in the file itself were already
            // recognised by the parser.
            return self.imported[source].contains(base)
                || self.defined[source]
                    .iter()
                    .any(|(name, defined)| name == base && *defined < line);
        };
        let Some(module) = self.bound_module(source, first) else {
            return false;
        };
        let (path, class_name) = match rest.rsplit_once('.') {
            Some((middle, class_name)) => (format!("{module}.{middle}"), class_name),
            None => (module, rest),
        };
        self.by_dotted
            .get(path.as_str())
            .is_some_and(|target| self.exports[*target].contains(class_name))
    }

    /// The module a name refers to in `source`: `import a.b` binds `a` to
    /// `a`, `import a.b as x` binds `x` to `a.b`, and `from a import b` binds
    /// `b` to `a.b` when `a.b` is a module. The last binding wins.
    fn bound_module(&self, source: usize, name: &str) -> Option<String> {
        self.sources[source]
            .imports
            .iter()
            .rev()
            .find_map(|import| {
                let position = import.bindings.iter().position(|binding| binding == name)?;
                let module = self.module(source, import)?;
                if import.names.is_empty() {
                    let first = module.split('.').next().unwrap_or(&module);
                    Some(if name == first {
                        first.to_string()
                    } else {
                        module
                    })
                } else {
                    Some(format!("{module}.{}", import.names[position]))
                }
            })
    }

    /// The absolute dotted module an import reads from.
    fn module(&self, source: usize, import: &Import) -> Option<String> {
        if import.level == 0 {
            return import.module.clone();
        }
        let current = &self.sources[source];
        let mut package = if current.is_package {
            current.dotted.as_str()
        } else {
            current.dotted.rsplit_once('.')?.0
        };
        for _ in 1..import.level {
            package = package.rsplit_once('.')?.0;
        }
        Some(match &import.module {
            Some(module) => format!("{package}.{module}"),
            None => package.to_string(),
        })
    }
}
