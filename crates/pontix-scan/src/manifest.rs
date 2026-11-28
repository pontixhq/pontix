//! What the modules being upgraded need from their own manifest and files to
//! load at all in the target version.

use std::num::NonZeroU32;

use pontix_core::{DiagnosticKind, Location, ModuleName};
use pontix_index::Addons;

use crate::{Evidence, Issue, Problem, Since};

/// Odoo treats a module whose version is of another series as not
/// installable, with only a log warning.
const VERSION_RULE: &str = "odoo/modules/module.py, adapt_version and the installable check";

/// The manifest file names Odoo reads.
const MANIFEST_RULE: &str = "odoo/modules/module.py, MANIFEST_NAMES";

/// Odoo reads a manifest with `ast.literal_eval`.
const LITERAL_RULE: &str = "odoo/modules/module.py, load_manifest (ast.literal_eval)";

pub(crate) fn check(source: &Addons, target: &Addons, upgrading: &[ModuleName]) -> Vec<Issue> {
    let mut issues = Vec::new();
    for module in &source.modules {
        if !upgrading.contains(&module.name) {
            continue;
        }
        let at = Location::new(&module.manifest_path, NonZeroU32::MIN);
        let in_target = target.modules.iter().any(|found| found.name == module.name);
        let manifest_file = module
            .manifest_path
            .file_name()
            .and_then(|name| name.to_str());
        if !in_target && manifest_file == Some("__openerp__.py") {
            issues.push(issue(
                &module.name,
                &at,
                Problem::ManifestFileIgnored,
                MANIFEST_RULE,
            ));
            continue;
        }
        let written = module.manifest.version.as_deref().unwrap_or("1.0");
        let fits = target
            .version
            .adapt_module_version(written)
            .is_some_and(|adapted| target.version.is_own_series(&adapted));
        if !fits {
            let problem = Problem::ManifestVersion {
                written: written.to_string(),
            };
            issues.push(issue(&module.name, &at, problem, VERSION_RULE));
        }
    }
    for diagnostic in &source.diagnostics {
        let problem = match diagnostic.kind {
            DiagnosticKind::UnrecognizedManifest => Problem::ManifestUnreadable,
            DiagnosticKind::UnresolvedImport => Problem::ImportBroken {
                import: diagnostic.snippet.clone(),
            },
            _ => continue,
        };
        // The module directory: the manifest's, or an ancestor of the file.
        let Some(module) = upgrading.iter().find(|name| {
            diagnostic
                .location
                .file
                .ancestors()
                .any(|dir| dir.file_name().is_some_and(|dir| dir == name.as_str()))
        }) else {
            continue;
        };
        let rule = match problem {
            Problem::ManifestUnreadable => LITERAL_RULE,
            _ => "Python import system: ImportError",
        };
        // Broken in the source version as much as in the target one.
        let mut found = issue(module, &diagnostic.location, problem, rule);
        found.since = Since::Before;
        issues.push(found);
    }
    issues
}

fn issue(module: &ModuleName, at: &Location, problem: Problem, rule: &'static str) -> Issue {
    Issue {
        module: module.clone(),
        at: at.clone(),
        problem,
        since: Since::Upgrade,
        evidence: Evidence {
            rule: Some(rule),
            ..Evidence::default()
        },
    }
}
