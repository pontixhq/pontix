//! Finding modules in addons paths.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use pontix_core::{Diagnostic, DiagnosticKind, Manifest, ModelDef, ModuleName, OdooVersion};
use pontix_parsers::parse_manifest;

use crate::files::{Root, whole_file};
use crate::load::load;

/// One Odoo module and what Odoo would load from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    /// Technical name: the directory name.
    pub name: ModuleName,
    /// The module directory, under the addons path it was found in.
    pub path: PathBuf,
    /// The manifest file Odoo reads for this module.
    pub manifest_path: PathBuf,
    /// The manifest, as written.
    pub manifest: Manifest,
    /// Python files Odoo imports, in import order.
    pub python_files: Vec<PathBuf>,
    /// Model classes from those files, in the order Python defines them.
    pub models: Vec<ModelDef>,
}

/// Every module found in a set of addons paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Addons {
    /// The Odoo version whose rules found these modules.
    pub version: OdooVersion,
    /// Modules, in addons-path order, then by name.
    pub modules: Vec<Module>,
    /// Everything that could not be read, across all modules.
    pub diagnostics: Vec<Diagnostic>,
}

/// Finds the modules in `addons_paths` as `version` would: a module is a
/// directory directly inside an addons path holding a manifest file. When a
/// name appears in several paths, the first one wins, as in Odoo.
pub fn scan(addons_paths: &[PathBuf], version: OdooVersion) -> Addons {
    let mut addons = Addons {
        version,
        modules: Vec::new(),
        diagnostics: Vec::new(),
    };
    let mut seen = HashSet::new();
    for addons_path in addons_paths {
        let root = match Root::new(addons_path) {
            Ok(root) => root,
            Err(diagnostic) => {
                addons.diagnostics.push(diagnostic);
                continue;
            }
        };
        let Ok(entries) = fs::read_dir(addons_path) else {
            addons
                .diagnostics
                .push(whole_file(DiagnosticKind::UnreadableFile, addons_path));
            continue;
        };
        let mut dirs: Vec<PathBuf> = entries
            .filter_map(|entry| Some(entry.ok()?.path()))
            .collect();
        dirs.sort();
        for dir in dirs {
            scan_module(&root, &dir, version, &mut seen, &mut addons);
        }
    }
    addons
}

fn scan_module(
    root: &Root,
    dir: &Path,
    version: OdooVersion,
    seen: &mut HashSet<ModuleName>,
    addons: &mut Addons,
) {
    match root.resolve(dir) {
        Ok(Some(canonical)) if canonical.is_dir() => {}
        Ok(_) => return,
        Err(diagnostic) => return addons.diagnostics.push(diagnostic),
    }
    let Some(manifest_path) = version
        .manifest_file_names()
        .iter()
        .map(|file_name| dir.join(file_name))
        .find(|path| path.is_file())
    else {
        return;
    };
    // A directory name that is not UTF-8 cannot be a Python package name.
    let Some(name) = dir.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let name = ModuleName::new(name);
    if !seen.insert(name.clone()) {
        return addons
            .diagnostics
            .push(whole_file(DiagnosticKind::DuplicateModule, &manifest_path));
    }
    let source = match root.read(&manifest_path) {
        Ok(source) => source,
        Err(diagnostic) => return addons.diagnostics.push(diagnostic),
    };
    let parsed = parse_manifest(&manifest_path, &source);
    addons.diagnostics.extend(parsed.diagnostics);
    // Odoo ignores a module whose manifest it cannot read.
    let Some(manifest) = parsed.manifest else {
        return;
    };
    let loaded = load(root, dir, &name);
    addons.diagnostics.extend(loaded.diagnostics);
    addons.modules.push(Module {
        name,
        path: dir.to_path_buf(),
        manifest_path,
        manifest,
        python_files: loaded.files,
        models: loaded.models,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MAX_FILE_SIZE;

    /// An addons path in a temporary directory, removed when dropped.
    struct Tree {
        dir: tempfile::TempDir,
    }

    impl Tree {
        fn new() -> Self {
            Self {
                dir: tempfile::tempdir().expect("a temporary directory"),
            }
        }

        fn path(&self) -> PathBuf {
            self.dir.path().to_path_buf()
        }

        fn write(&self, relative: &str, content: impl AsRef<[u8]>) -> &Self {
            let path = self.dir.path().join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("parent directories");
            }
            fs::write(path, content).expect("a written file");
            self
        }

        fn module(&self, name: &str, init: &str) -> &Self {
            self.write(&format!("{name}/__manifest__.py"), "{'name': 'Test'}")
                .write(&format!("{name}/__init__.py"), init)
        }

        fn scan(&self, version: OdooVersion) -> Addons {
            scan(&[self.path()], version)
        }
    }

    fn relative(tree: &Tree, paths: &[PathBuf]) -> Vec<String> {
        paths
            .iter()
            .map(|path| {
                path.strip_prefix(tree.dir.path())
                    .expect("inside the tree")
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }

    fn class_names(module: &Module) -> Vec<&str> {
        module
            .models
            .iter()
            .map(|m| m.class_name.as_str())
            .collect()
    }

    fn kinds(addons: &Addons) -> Vec<DiagnosticKind> {
        addons.diagnostics.iter().map(|d| d.kind).collect()
    }

    #[test]
    fn reads_only_files_odoo_imports() {
        let tree = Tree::new();
        tree.module("sale_custom", "from . import models\n")
            .write(
                "sale_custom/models/__init__.py",
                "from . import sale_order\n",
            )
            .write(
                "sale_custom/models/sale_order.py",
                "class SaleOrder(models.Model):\n    _inherit = 'sale.order'\n",
            )
            .write(
                "sale_custom/models/ghost.py",
                "class Ghost(models.Model):\n    _inherit = 'ir.attachment'\n",
            )
            .write("not_a_module/README.md", "no manifest here");
        let addons = tree.scan(OdooVersion::V19);
        assert!(addons.diagnostics.is_empty(), "{:?}", addons.diagnostics);
        assert_eq!(addons.modules.len(), 1);
        let module = &addons.modules[0];
        assert_eq!(module.name.as_str(), "sale_custom");
        assert_eq!(module.manifest.name.as_deref(), Some("Test"));
        assert_eq!(
            relative(&tree, &module.python_files),
            [
                "sale_custom/__init__.py",
                "sale_custom/models/__init__.py",
                "sale_custom/models/sale_order.py",
            ]
        );
        assert_eq!(class_names(module), ["SaleOrder"]);
    }

    #[test]
    fn models_follow_python_execution_order() {
        let tree = Tree::new();
        tree.module(
            "m",
            "class A(models.Model): pass\nfrom . import b\nclass C(models.Model): pass\n",
        )
        .write("m/b.py", "class B(models.Model): pass\n");
        let addons = tree.scan(OdooVersion::V19);
        assert_eq!(class_names(&addons.modules[0]), ["A", "B", "C"]);
    }

    #[test]
    fn follows_from_package_imports_and_absolute_self_imports() {
        let tree = Tree::new();
        tree.module(
            "m",
            "from .models import sale_order\nfrom odoo.addons.m import wizard\n",
        )
        .write("m/models/__init__.py", "")
        .write(
            "m/models/sale_order.py",
            "class SaleOrder(models.Model): pass\n",
        )
        .write("m/wizard.py", "class Wizard(models.TransientModel): pass\n");
        let addons = tree.scan(OdooVersion::V19);
        assert!(addons.diagnostics.is_empty(), "{:?}", addons.diagnostics);
        assert_eq!(class_names(&addons.modules[0]), ["SaleOrder", "Wizard"]);
    }

    #[test]
    fn imports_of_other_modules_and_attributes_are_not_errors() {
        let tree = Tree::new();
        tree.module(
            "m",
            "from odoo import models\nfrom odoo.addons.sale.models import sale_order\nfrom .hooks import post_init_hook\nfrom . import models as m\n",
        )
        .write("m/hooks.py", "def post_init_hook(env): pass\n")
        .write("m/models/__init__.py", "");
        let addons = tree.scan(OdooVersion::V19);
        assert!(addons.diagnostics.is_empty(), "{:?}", addons.diagnostics);
    }

    #[test]
    fn a_missing_import_target_is_reported_verbatim() {
        let tree = Tree::new();
        tree.module("m", "from . import models\nfrom .wizard import helper\n")
            .write("m/models/__init__.py", "");
        let addons = tree.scan(OdooVersion::V19);
        let seen: Vec<(DiagnosticKind, &str)> = addons
            .diagnostics
            .iter()
            .map(|d| (d.kind, d.snippet.as_str()))
            .collect();
        assert_eq!(
            seen,
            [(
                DiagnosticKind::UnresolvedImport,
                "from .wizard import helper"
            )]
        );
    }

    #[test]
    fn names_bound_in_the_package_init_are_not_missing_files() {
        let tree = Tree::new();
        tree.module("m", "from . import controllers\n")
            .write(
                "m/controllers/__init__.py",
                "NOTICE = 'deprecated'\nfrom . import jsonrpc\n",
            )
            .write("m/controllers/jsonrpc.py", "from . import NOTICE\n");
        let addons = tree.scan(OdooVersion::V19);
        assert!(addons.diagnostics.is_empty(), "{:?}", addons.diagnostics);
    }

    #[test]
    fn a_directory_without_init_is_a_namespace_package() {
        let tree = Tree::new();
        tree.module("m", "from . import demo\n")
            .write("m/demo/data.xml", "<odoo/>");
        let addons = tree.scan(OdooVersion::V19);
        assert!(addons.diagnostics.is_empty(), "{:?}", addons.diagnostics);
    }

    #[test]
    fn fallback_imports_in_except_handlers_are_not_followed() {
        let tree = Tree::new();
        tree.module(
            "m",
            "try:\n    from werkzeug.utils import send_file\nexcept ImportError:\n    from .vendor import send_file\n",
        );
        let addons = tree.scan(OdooVersion::V19);
        assert!(addons.diagnostics.is_empty(), "{:?}", addons.diagnostics);
    }

    #[test]
    fn openerp_manifests_are_read_by_18_only() {
        let tree = Tree::new();
        tree.write("legacy/__openerp__.py", "{'name': 'Legacy'}");
        assert_eq!(tree.scan(OdooVersion::V18).modules.len(), 1);
        assert!(tree.scan(OdooVersion::V19).modules.is_empty());
    }

    #[test]
    fn an_unreadable_manifest_hides_the_module() {
        let tree = Tree::new();
        tree.write("m/__manifest__.py", "{'name': NAME}");
        let addons = tree.scan(OdooVersion::V19);
        assert!(addons.modules.is_empty());
        assert_eq!(kinds(&addons), [DiagnosticKind::UnrecognizedManifest]);
    }

    #[test]
    fn the_first_addons_path_wins() {
        let first = Tree::new();
        first.module("m", "");
        let second = Tree::new();
        second.module("m", "");
        let addons = scan(&[first.path(), second.path()], OdooVersion::V19);
        assert_eq!(addons.modules.len(), 1);
        assert!(addons.modules[0].path.starts_with(first.dir.path()));
        assert_eq!(kinds(&addons), [DiagnosticKind::DuplicateModule]);
    }

    #[test]
    fn oversized_and_non_utf8_files_are_reported_not_read() {
        let tree = Tree::new();
        let oversized = usize::try_from(MAX_FILE_SIZE + 1).expect("fits in memory");
        tree.module("m", "from . import big, latin\n")
            .write("m/big.py", vec![b' '; oversized])
            .write("m/latin.py", b"# caf\xe9\n");
        let addons = tree.scan(OdooVersion::V19);
        assert_eq!(
            kinds(&addons),
            [DiagnosticKind::FileTooLarge, DiagnosticKind::UnreadableFile]
        );
        assert_eq!(addons.diagnostics[0].location.line.get(), 1);
        assert!(addons.diagnostics[0].snippet.is_empty());
    }

    #[test]
    fn a_byte_order_mark_is_ignored() {
        let tree = Tree::new();
        tree.write("m/__manifest__.py", "\u{feff}{'name': 'Bom'}")
            .write("m/__init__.py", "");
        let addons = tree.scan(OdooVersion::V19);
        assert!(addons.diagnostics.is_empty(), "{:?}", addons.diagnostics);
        assert_eq!(addons.modules[0].manifest.name.as_deref(), Some("Bom"));
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_links_out_of_the_addons_path_are_not_followed() {
        let outside = Tree::new();
        outside.write("secret.py", "class Secret(models.Model): pass\n");
        let tree = Tree::new();
        tree.module("m", "from . import secret\n");
        std::os::unix::fs::symlink(
            outside.dir.path().join("secret.py"),
            tree.dir.path().join("m/secret.py"),
        )
        .expect("a symbolic link");
        let addons = tree.scan(OdooVersion::V19);
        assert!(addons.modules[0].models.is_empty());
        assert_eq!(kinds(&addons), [DiagnosticKind::OutsideRoot]);
    }
}
