//! `pontix plan`: checks the inputs, scans, and writes the report.

use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use pontix_core::{ModuleName, OdooVersion, Severity};
use pontix_parsers::release_version;

use crate::output::{self, Notice, Roots};
use crate::{FailOn, Format, PlanArgs};

/// Largest `odoo/release.py` Pontix reads. The real one is a few KiB.
const MAX_RELEASE_SIZE: u64 = 1024 * 1024;

/// Exit code for input Pontix cannot work with.
pub(crate) fn input_error() -> ExitCode {
    ExitCode::from(2)
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Outcome {
    /// Whether a finding reached `--fail-on`.
    pub(crate) failed: bool,
}

impl Outcome {
    /// 0, or 1 when a finding reached `--fail-on`.
    pub(crate) fn exit_code(self) -> ExitCode {
        if self.failed {
            ExitCode::from(1)
        } else {
            ExitCode::SUCCESS
        }
    }
}

/// Runs the plan and writes the report where `--output` says.
pub(crate) fn run(args: &PlanArgs) -> Result<Outcome, String> {
    let color = args.output.is_none()
        && args.format == Format::Text
        && std::io::stdout().is_terminal()
        && std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty());
    let (rendered, outcome) = plan(args, color)?;
    match &args.output {
        Some(file) => fs::write(file, rendered)
            .map_err(|error| format!("cannot write {}: {error}", file.display()))?,
        None => print!("{rendered}"),
    }
    Ok(outcome)
}

/// Checks the inputs, scans, and renders the report.
pub(crate) fn plan(args: &PlanArgs, color: bool) -> Result<(String, Outcome), String> {
    if args.from.next() != Some(args.to) {
        let next = args
            .from
            .next()
            .map_or("nothing yet".to_string(), |next| next.series().to_string());
        return Err(format!(
            "Pontix upgrades one version at a time: from {} it goes to {next}, not {}",
            args.from.series(),
            args.to.series()
        ));
    }
    check_core(&args.core_from, args.from, "--core-from")?;
    check_core(&args.core_to, args.to, "--core-to")?;
    for dir in &args.addons {
        if !dir.is_dir() {
            return Err(format!("--addons {} is not a folder", dir.display()));
        }
    }
    let found = custom_modules(&args.addons, args.from);
    let upgrading: Vec<ModuleName> = if args.modules.is_empty() {
        found
    } else {
        let mut chosen = Vec::new();
        for name in &args.modules {
            let name = ModuleName::new(name.trim());
            if !found.contains(&name) {
                return Err(format!("module {name} is not in the --addons folders"));
            }
            chosen.push(name);
        }
        chosen
    };
    if upgrading.is_empty() {
        return Err("no Odoo module in the --addons folders".to_string());
    }

    eprintln!(
        "pontix: checking {} module(s) for Odoo {} -> {}",
        upgrading.len(),
        args.from.series(),
        args.to.series()
    );
    let paths = |core: &Path| -> Vec<PathBuf> {
        [core.join("odoo").join("addons"), core.join("addons")]
            .into_iter()
            .filter(|path| path.is_dir())
            .chain(args.addons.iter().cloned())
            .collect()
    };
    let source = pontix_index::scan(&paths(&args.core_from), args.from);
    let target = pontix_index::scan(&paths(&args.core_to), args.to);
    let scan = pontix_scan::scan(&source, &target, &upgrading, &[]);
    let report = pontix_scan::report(&scan);
    let notices = notices(&source, &upgrading, args.from);

    let roots = Roots {
        addons: &args.addons,
        core_from: &args.core_from,
        core_to: &args.core_to,
        from: args.from,
        to: args.to,
    };
    let rendered = match args.format {
        Format::Text => output::text(&report, &upgrading, &notices, &roots, color),
        Format::Json => output::json(&report, &upgrading, &notices, &roots),
    };
    let failed = args.fail_on.is_some_and(|level| {
        let worst = match level {
            FailOn::Error => Severity::Error,
            FailOn::Warning => Severity::Warning,
        };
        report
            .findings
            .iter()
            .any(|finding| finding.severity <= worst)
    });
    Ok((rendered, Outcome { failed }))
}

/// The tree must hold the Odoo version the flag says, as its
/// `odoo/release.py` tells.
fn check_core(core: &Path, expected: OdooVersion, flag: &str) -> Result<(), String> {
    let release = core.join("odoo").join("release.py");
    let not_odoo = || {
        format!(
            "{flag} {} is not an Odoo source tree: odoo/release.py is missing or unreadable",
            core.display()
        )
    };
    let size = fs::metadata(&release).map_err(|_| not_odoo())?.len();
    if size > MAX_RELEASE_SIZE {
        return Err(not_odoo());
    }
    let text = fs::read_to_string(&release).map_err(|_| not_odoo())?;
    let found =
        release_version(&text).and_then(|(major, minor)| OdooVersion::from_release(major, minor));
    match found {
        Some(version) if version == expected => Ok(()),
        Some(version) => Err(format!(
            "{flag} {} holds Odoo {}, not {}",
            core.display(),
            version.series(),
            expected.series()
        )),
        None => Err(format!(
            "{flag} {}: Pontix cannot tell which Odoo version odoo/release.py describes",
            core.display()
        )),
    }
}

/// Every module directory in the addons folders: one with a manifest Odoo
/// reads in `version`, readable or not, in folder order and then by name.
fn custom_modules(addons: &[PathBuf], version: OdooVersion) -> Vec<ModuleName> {
    let mut found = Vec::new();
    for dir in addons {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        let mut names: Vec<String> = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_dir())
            .filter(|entry| {
                version
                    .manifest_file_names()
                    .iter()
                    .any(|manifest| entry.path().join(manifest).is_file())
            })
            .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
            .collect();
        names.sort();
        for name in names {
            let name = ModuleName::new(name);
            if !found.contains(&name) {
                found.push(name);
            }
        }
    }
    found
}

/// Modules whose manifest version is not of the version they upgrade from:
/// they may not be migrated to it yet.
fn notices(
    source: &pontix_index::Addons,
    upgrading: &[ModuleName],
    from: OdooVersion,
) -> Vec<Notice> {
    source
        .modules
        .iter()
        .filter(|module| upgrading.contains(&module.name))
        .filter_map(|module| {
            let written = module.manifest.version.as_deref().unwrap_or("1.0");
            let fits = from
                .adapt_module_version(written)
                .is_some_and(|adapted| from.is_own_series(&adapted));
            (!fits).then(|| Notice {
                module: module.name.clone(),
                message: format!(
                    "manifest version {written} is not a {} version: the module may not be migrated to {} yet",
                    from.series(),
                    from.series()
                ),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
