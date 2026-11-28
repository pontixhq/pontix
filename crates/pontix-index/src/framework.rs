//! The Odoo framework of one version: the Python modules under `odoo/`
//! other than `odoo/addons`, and the methods every model inherits from
//! `BaseModel`.
//!
//! A name is looked up the way Python imports it, through `from x import y`
//! re-exports and `import *`. Where Python could still find a name that the
//! source does not show, such as a namespace package that sets attributes
//! at run time or a module-level `__getattr__`, the answer is
//! [`Lookup::Unknown`], never [`Lookup::Missing`].

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use pontix_core::Location;
use pontix_parsers::{BindingKind, FrameworkClass, FrameworkFile, parse_framework};

use crate::files::Root;

/// How deep re-exports are followed before giving up.
const MAX_HOPS: usize = 16;

/// How deep directories under `odoo/` are read.
const MAX_DEPTH: usize = 8;

/// The framework of one version.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Framework {
    /// Each module by dotted name, with whether it is a package that has an
    /// `__init__.py`.
    modules: BTreeMap<String, Package>,
    /// Members of `BaseModel`: every model has them.
    base_model: BTreeMap<String, Api>,
    /// The field classes by name: `Field` and its subclasses, in
    /// `odoo.fields`, or in `odoo.orm.fields*` since 19.0.
    fields: BTreeMap<String, FrameworkClass>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Package {
    file: FrameworkFile,
    /// A directory without `__init__.py`: a namespace package, whose other
    /// attributes are only known at run time.
    namespace: bool,
    location: Location,
}

/// A name the framework has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Api {
    /// Where it is defined.
    pub location: Location,
    /// Where core marks it deprecated.
    pub deprecated: Option<Location>,
    /// Whether it is a module, whose attributes can be looked up in turn.
    pub module: bool,
}

/// What looking a name up found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    /// It exists.
    Found(Api),
    /// The source shows the name cannot exist.
    Missing,
    /// It may exist at run time.
    Unknown,
}

impl Framework {
    /// Whether nothing was read, as when no framework source was given.
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    /// Looks up a dotted name: a module such as `odoo.osv.expression`, or a
    /// name in one, such as `odoo.tools.ormcache_context`.
    pub fn lookup(&self, dotted: &str) -> Lookup {
        if let Some(package) = self.modules.get(dotted) {
            return Lookup::Found(Api {
                location: package.location.clone(),
                deprecated: package.file.deprecated.clone(),
                module: true,
            });
        }
        let Some((parent, name)) = dotted.rsplit_once('.') else {
            return Lookup::Missing;
        };
        if self.modules.contains_key(parent) {
            self.name(parent, name, 0)
        } else {
            Lookup::Unknown
        }
    }

    /// A method or attribute every model inherits from `BaseModel`.
    pub fn base_model(&self, name: &str) -> Option<&Api> {
        self.base_model.get(name)
    }

    /// Whether the field class of a type (`Many2one`) knows a parameter:
    /// an attribute or method of it or of a field class it derives from,
    /// which is what Odoo's `hasattr` test on the field asks, or a named
    /// parameter of one's `__init__`, which takes it in. Any other
    /// parameter Odoo ignores with a warning. Unknown for a type the
    /// framework does not define, or a base it cannot follow.
    pub fn field_parameter(&self, field_type: &str, name: &str) -> Lookup {
        if self.fields.is_empty() {
            return Lookup::Unknown;
        }
        let mut pending = vec![field_type];
        let mut seen = BTreeSet::new();
        while let Some(class) = pending.pop() {
            if !seen.insert(class) {
                continue;
            }
            let Some(found) = self.fields.get(class) else {
                // Typing helpers add nothing a field knows.
                if matches!(class, "Generic" | "object") {
                    continue;
                }
                return Lookup::Unknown;
            };
            let init = found
                .members
                .iter()
                .find(|member| member.name == "__init__");
            let member = found
                .members
                .iter()
                .find(|member| member.name == name)
                .or(init.filter(|_| found.init_params.iter().any(|param| param == name)));
            if let Some(member) = member {
                return Lookup::Found(Api {
                    location: member.location.clone(),
                    deprecated: member.deprecated.clone(),
                    module: false,
                });
            }
            pending.extend(found.bases.iter().map(String::as_str));
        }
        Lookup::Missing
    }

    fn name(&self, module: &str, name: &str, hops: usize) -> Lookup {
        if hops > MAX_HOPS {
            return Lookup::Unknown;
        }
        let Some(package) = self.modules.get(module) else {
            return Lookup::Unknown;
        };
        // The last binding is the one Python keeps.
        if let Some(binding) = package
            .file
            .bindings
            .iter()
            .rev()
            .find(|binding| binding.name == name)
        {
            return match &binding.kind {
                BindingKind::Def { deprecated } => Lookup::Found(Api {
                    location: binding.location.clone(),
                    deprecated: deprecated.clone(),
                    module: false,
                }),
                BindingKind::From {
                    module: from,
                    name: original,
                } => {
                    let submodule = format!("{from}.{original}");
                    if self.modules.contains_key(&submodule) {
                        self.lookup(&submodule)
                    } else if self.modules.contains_key(from) {
                        self.name(from, original, hops + 1)
                    } else {
                        Lookup::Unknown
                    }
                }
                BindingKind::Module { module } => match self.modules.contains_key(module) {
                    true => self.lookup(module),
                    false => Lookup::Unknown,
                },
            };
        }
        let submodule = format!("{module}.{name}");
        if self.modules.contains_key(&submodule) {
            return self.lookup(&submodule);
        }
        for star in &package.file.stars {
            if let Lookup::Found(api) = self.name(star, name, hops + 1) {
                return Lookup::Found(api);
            }
        }
        if package.namespace || package.file.dynamic || !package.file.parsed {
            return Lookup::Unknown;
        }
        if package
            .file
            .stars
            .iter()
            .any(|star| !self.modules.contains_key(star))
        {
            return Lookup::Unknown;
        }
        Lookup::Missing
    }
}

/// Reads the framework of the Odoo source tree at `core`: `core/odoo`,
/// without `core/odoo/addons`. An empty framework when there is none.
pub fn framework(core: &Path) -> Framework {
    let odoo = core.join("odoo");
    let Ok(root) = Root::new(&odoo) else {
        return Framework::default();
    };
    let mut out = Framework::default();
    read_dir(&root, &odoo, "odoo", 0, &mut out);
    let fields: BTreeMap<String, FrameworkClass> = out
        .modules
        .iter()
        .filter(|(dotted, _)| *dotted == "odoo.fields" || dotted.starts_with("odoo.orm.fields"))
        .flat_map(|(_, package)| &package.file.classes)
        .map(|class| (class.name.clone(), class.clone()))
        .collect();
    out.fields = fields;
    let base_model = out
        .modules
        .values()
        .flat_map(|package| &package.file.classes)
        .find(|class| class.name == "BaseModel" && !class.members.is_empty());
    if let Some(class) = base_model {
        out.base_model = class
            .members
            .iter()
            .map(|member| {
                (
                    member.name.clone(),
                    Api {
                        location: member.location.clone(),
                        deprecated: member.deprecated.clone(),
                        module: false,
                    },
                )
            })
            .collect();
    }
    out
}

fn read_dir(root: &Root, dir: &Path, dotted: &str, depth: usize, out: &mut Framework) {
    if depth > MAX_DEPTH {
        return;
    }
    let init = dir.join("__init__.py");
    let namespace = !init.is_file();
    let file = if namespace {
        FrameworkFile {
            parsed: true,
            ..FrameworkFile::default()
        }
    } else {
        read_file(root, &init, dotted, true)
    };
    out.modules.insert(
        dotted.to_string(),
        Package {
            file,
            namespace,
            location: Location::new(if namespace { dir } else { init.as_path() }, line_one()),
        },
    );
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| Some(entry.ok()?.path()))
        .collect();
    paths.sort();
    for path in paths {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        // Addons are modules, read by the indexer; `__pycache__` and the
        // like are not Python packages.
        if (depth == 0 && name == "addons") || name.starts_with(['_', '.']) && name != "__init__.py"
        {
            continue;
        }
        if !matches!(root.resolve(&path), Ok(Some(_))) {
            continue;
        }
        if path.is_dir() {
            read_dir(root, &path, &format!("{dotted}.{name}"), depth + 1, out);
        } else if let Some(stem) = name.strip_suffix(".py")
            && name != "__init__.py"
        {
            let module = format!("{dotted}.{stem}");
            let file = read_file(root, &path, &module, false);
            out.modules.insert(
                module,
                Package {
                    file,
                    namespace: false,
                    location: Location::new(&path, line_one()),
                },
            );
        }
    }
}

fn read_file(root: &Root, path: &Path, dotted: &str, package: bool) -> FrameworkFile {
    match root.read(path) {
        Ok(source) => parse_framework(path, &source, dotted, package),
        Err(_) => FrameworkFile::default(),
    }
}

fn line_one() -> std::num::NonZeroU32 {
    std::num::NonZeroU32::MIN
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, relative: &str, content: &str) {
        let path = dir.join(relative);
        fs::create_dir_all(path.parent().expect("a parent")).expect("directories");
        fs::write(path, content).expect("a file");
    }

    fn core() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path();
        write(root, "odoo/init.py", "import odoo\nodoo.Command = 1\n");
        write(
            root,
            "odoo/tools/__init__.py",
            "from .cache import ormcache, ormcache_context\nfrom .misc import *\n",
        );
        write(
            root,
            "odoo/tools/cache.py",
            "import warnings\nclass ormcache: pass\nclass ormcache_context(ormcache):\n    def __init__(self):\n        warnings.warn('x', DeprecationWarning)\n",
        );
        write(root, "odoo/tools/misc.py", "def float_round(x): pass\n");
        write(
            root,
            "odoo/orm/models.py",
            "class BaseModel:\n    def read_group(self):\n        \"\"\"Deprecated - use _read_group\"\"\"\n    def search(self): pass\n",
        );
        write(root, "odoo/addons/base/__init__.py", "x = 1\n");
        dir
    }

    #[test]
    fn looks_names_up_through_reexports_and_star_imports() {
        let core = core();
        let framework = framework(core.path());
        let found = |name: &str| match framework.lookup(name) {
            Lookup::Found(api) => Some(api.deprecated.is_some()),
            Lookup::Missing => None,
            Lookup::Unknown => Some(false),
        };
        assert_eq!(found("odoo.tools.ormcache"), Some(false));
        assert_eq!(found("odoo.tools.ormcache_context"), Some(true));
        assert_eq!(found("odoo.tools.float_round"), Some(false));
        assert_eq!(found("odoo.tools.cache"), Some(false));
        assert_eq!(found("odoo.tools.gone"), None);
        // A namespace package may get attributes at run time.
        assert_eq!(framework.lookup("odoo.Command"), Lookup::Unknown);
        assert_eq!(framework.lookup("odoo.tools.ormcache.x"), Lookup::Unknown);
        assert_eq!(framework.lookup("odoo.addons.base"), Lookup::Unknown);
        assert!(
            framework
                .base_model("read_group")
                .is_some_and(|api| api.deprecated.is_some())
        );
        assert!(
            framework
                .base_model("search")
                .is_some_and(|api| api.deprecated.is_none())
        );
        assert!(framework.base_model("_where_calc").is_none());
    }

    #[test]
    fn no_framework_source_gives_an_empty_framework() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        assert!(framework(dir.path()).is_empty());
    }

    #[test]
    fn field_parameters_follow_the_field_classes() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path();
        write(root, "odoo/__init__.py", "");
        write(root, "odoo/orm/__init__.py", "");
        write(
            root,
            "odoo/orm/fields.py",
            "import typing\nclass MetaField(type): pass\nclass Field(MetaField('Dummy', (object,), {}), typing.Generic[T]):\n    string: str = None\n    def __init__(self, string=None, **kwargs): pass\n",
        );
        write(
            root,
            "odoo/orm/fields_relational.py",
            "from .fields import Field\nclass _Relational(Field[M]):\n    bypass_search_access = False\nclass Many2one(_Relational):\n    def __init__(self, comodel_name=None, *, ondelete=None, **kwargs): pass\nclass Odd(Unknown):\n    pass\n",
        );
        let found = framework(root);
        let known = |field_type: &str, name: &str| match found.field_parameter(field_type, name) {
            Lookup::Found(_) => "found",
            Lookup::Missing => "missing",
            Lookup::Unknown => "unknown",
        };
        assert_eq!(known("Many2one", "bypass_search_access"), "found");
        assert_eq!(known("Many2one", "string"), "found");
        assert_eq!(
            known("Many2one", "ondelete"),
            "found",
            "an __init__ parameter"
        );
        assert_eq!(known("Many2one", "auto_join"), "missing");
        assert_eq!(known("Many2one", "kwargs"), "missing");
        assert_eq!(
            known("Odd", "anything"),
            "unknown",
            "a base it cannot follow"
        );
        assert_eq!(
            known("Custom", "string"),
            "unknown",
            "a type it does not define"
        );
        assert_eq!(
            Framework::default().field_parameter("Char", "size"),
            Lookup::Unknown
        );
    }
}
