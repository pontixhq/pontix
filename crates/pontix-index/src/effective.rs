//! The manifest as a given Odoo version understands it: defaults, the
//! `auto_install` triggers and the version check (`odoo/modules/module.py`).

use pontix_core::{AutoInstall, Diagnostic, DiagnosticKind, ModuleName, OdooVersion};

use crate::files::whole_file;
use crate::scan::Module;

/// What Odoo derives from a manifest before installing the module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Effective {
    pub(crate) depends: Vec<ModuleName>,
    /// Modules whose installation installs this one; `None` when the module
    /// is never installed automatically.
    pub(crate) auto_install: Option<Vec<ModuleName>>,
}

/// Applies the manifest rules of `version`, or explains why Odoo would not
/// install the module at all.
pub(crate) fn effective(module: &Module, version: OdooVersion) -> Result<Effective, Diagnostic> {
    effective_for(module, version, false)
}

/// [`effective`], with `any_series` accepting a manifest version of another
/// series, as for a module still written for the version it upgrades from.
pub(crate) fn effective_for(
    module: &Module,
    version: OdooVersion,
    any_series: bool,
) -> Result<Effective, Diagnostic> {
    let manifest = &module.manifest;
    let fail = |kind, snippet: &str| Diagnostic {
        snippet: snippet.to_string(),
        ..whole_file(kind, &module.manifest_path)
    };

    let base = ModuleName::new("base");
    let depends = if !version.empty_depends_means_base() {
        manifest.depends.clone()
    } else if module.name == base {
        Vec::new()
    } else if manifest.depends.is_empty() {
        vec![base]
    } else {
        manifest.depends.clone()
    };

    let auto_install = match &manifest.auto_install {
        AutoInstall::No => None,
        AutoInstall::WithAllDepends => Some(depends.clone()),
        AutoInstall::When(triggers) => {
            if let Some(stray) = triggers.iter().find(|trigger| !depends.contains(trigger)) {
                return Err(fail(DiagnosticKind::InvalidAutoInstall, stray.as_str()));
            }
            Some(triggers.clone())
        }
    };

    let installable = manifest.installable.unwrap_or(true);
    let written = manifest.version.as_deref().unwrap_or("1.0");
    match version.adapt_module_version(written) {
        // An invalid version only matters when Odoo would install the module.
        None if installable => return Err(fail(DiagnosticKind::InvalidVersion, written)),
        _ if !installable => return Err(fail(DiagnosticKind::NotInstallable, "")),
        Some(adapted) if !any_series && !version.is_own_series(&adapted) => {
            return Err(fail(DiagnosticKind::OtherSeriesVersion, written));
        }
        _ => {}
    }

    Ok(Effective {
        depends,
        auto_install,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pontix_core::Manifest;

    use super::*;

    fn module(name: &str, manifest: Manifest) -> Module {
        Module {
            name: ModuleName::new(name),
            path: PathBuf::from(name),
            manifest_path: PathBuf::from(format!("{name}/__manifest__.py")),
            manifest,
            python_files: Vec::new(),
            models: Vec::new(),
            xmlid_refs: Vec::new(),
            code_refs: Vec::new(),
            test_refs: Vec::new(),
            data: Vec::new(),
        }
    }

    fn names(values: &[&str]) -> Vec<ModuleName> {
        values.iter().copied().map(ModuleName::new).collect()
    }

    fn with_version(version: &str) -> Manifest {
        Manifest {
            version: Some(version.to_string()),
            ..Manifest::default()
        }
    }

    fn failure(module: &Module, version: OdooVersion) -> (DiagnosticKind, String) {
        let diagnostic = effective(module, version).expect_err("Odoo would not install it");
        (diagnostic.kind, diagnostic.snippet)
    }

    #[test]
    fn empty_depends_means_base_in_19_only() {
        let m = module("m", Manifest::default());
        assert_eq!(
            effective(&m, OdooVersion::V19).map(|e| e.depends),
            Ok(names(&["base"]))
        );
        assert_eq!(
            effective(&m, OdooVersion::V18).map(|e| e.depends),
            Ok(Vec::new())
        );
        let base = module("base", Manifest::default());
        assert_eq!(
            effective(&base, OdooVersion::V19).map(|e| e.depends),
            Ok(Vec::new())
        );
    }

    #[test]
    fn auto_install_true_means_every_dependency() {
        let m = module(
            "sale_stock",
            Manifest {
                depends: names(&["sale", "stock"]),
                auto_install: AutoInstall::WithAllDepends,
                ..Manifest::default()
            },
        );
        assert_eq!(
            effective(&m, OdooVersion::V19).map(|e| e.auto_install),
            Ok(Some(names(&["sale", "stock"])))
        );
    }

    #[test]
    fn auto_install_triggers_must_be_dependencies() {
        let m = module(
            "m",
            Manifest {
                depends: names(&["sale"]),
                auto_install: AutoInstall::When(names(&["sale", "stock"])),
                ..Manifest::default()
            },
        );
        assert_eq!(
            failure(&m, OdooVersion::V19),
            (DiagnosticKind::InvalidAutoInstall, "stock".to_string())
        );
    }

    #[test]
    fn versions_of_the_series_and_short_versions_are_accepted() {
        for (version, written) in [
            (OdooVersion::V19, "19.0.1.0.0"),
            (OdooVersion::V19, "1.0"),
            (OdooVersion::V19, "1.2.3"),
            (OdooVersion::V18, "18.0.1.0.0"),
            (OdooVersion::V18, "1.0"),
        ] {
            let m = module("m", with_version(written));
            assert!(effective(&m, version).is_ok(), "{version:?} {written}");
        }
    }

    #[test]
    fn another_series_is_uninstallable_in_19_and_invalid_in_18() {
        let m = module("m", with_version("18.0.1.0.0"));
        assert_eq!(
            failure(&m, OdooVersion::V19),
            (DiagnosticKind::OtherSeriesVersion, "18.0.1.0.0".to_string())
        );
        let m = module("m", with_version("17.0.1.0.0"));
        assert_eq!(
            failure(&m, OdooVersion::V18),
            (DiagnosticKind::InvalidVersion, "17.0.1.0.0".to_string())
        );
    }

    #[test]
    fn malformed_versions_are_invalid() {
        for (version, written) in [
            (OdooVersion::V19, "1"),
            (OdooVersion::V19, "1.0.0.0.0.0"),
            (OdooVersion::V19, "1.x"),
            (OdooVersion::V18, "1.0.0.0"),
            (OdooVersion::V18, "v1.0"),
        ] {
            let m = module("m", with_version(written));
            assert_eq!(
                failure(&m, version),
                (DiagnosticKind::InvalidVersion, written.to_string()),
                "{version:?} {written}"
            );
        }
    }

    #[test]
    fn not_installable_wins_over_a_bad_version() {
        let m = module(
            "m",
            Manifest {
                version: Some("garbage".to_string()),
                installable: Some(false),
                ..Manifest::default()
            },
        );
        assert_eq!(
            failure(&m, OdooVersion::V19),
            (DiagnosticKind::NotInstallable, String::new())
        );
    }
}
