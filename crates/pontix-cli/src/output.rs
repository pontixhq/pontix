//! The report, for people (text) and for tools (JSON). Paths are shown
//! relative to the folder they were found in, so a shared report does not
//! carry the user's own absolute paths.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use pontix_core::{Basis, Category, Finding, Location, ModuleName, OdooVersion, Severity};
use pontix_scan::Report;
use serde::Serialize;

/// The JSON schema version. Raised whenever a change can break a tool that
/// reads the report.
pub(crate) const SCHEMA_VERSION: u32 = 1;

/// How many places of one finding the text report lists.
const SHOWN_LOCATIONS: usize = 3;

/// Something about the run worth saying that is not a finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Notice {
    /// The module it is about.
    pub(crate) module: ModuleName,
    /// What to know.
    pub(crate) message: String,
}

/// The folders paths are shown relative to.
pub(crate) struct Roots<'a> {
    pub(crate) addons: &'a [PathBuf],
    pub(crate) core_from: &'a Path,
    pub(crate) core_to: &'a Path,
    pub(crate) from: OdooVersion,
    pub(crate) to: OdooVersion,
}

/// Which folder a path is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RootKind {
    Addons,
    CoreFrom,
    CoreTo,
    Other,
}

impl Roots<'_> {
    fn place(&self, path: &Path) -> (RootKind, String) {
        let roots = self
            .addons
            .iter()
            .map(|root| (RootKind::Addons, root.as_path()))
            .chain([
                (RootKind::CoreFrom, self.core_from),
                (RootKind::CoreTo, self.core_to),
            ]);
        for (kind, root) in roots {
            if let Ok(relative) = path.strip_prefix(root) {
                return (kind, slashes(relative));
            }
        }
        // Not under a folder the user gave: keep only the file name.
        let name = path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        (RootKind::Other, name)
    }

    fn text(&self, location: &Location) -> String {
        let (kind, path) = self.place(&location.file);
        match kind {
            RootKind::CoreFrom => format!("[{}] {path}:{}", self.from.series(), location.line),
            RootKind::CoreTo => format!("[{}] {path}:{}", self.to.series(), location.line),
            RootKind::Addons | RootKind::Other => format!("{path}:{}", location.line),
        }
    }
}

fn slashes(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// A finding about a module being upgraded, as opposed to what core does to
/// data in every database that installs it.
fn in_custom(finding: &Finding, upgrading: &[ModuleName]) -> bool {
    upgrading.contains(&finding.module)
}

fn counts<'f>(findings: impl Iterator<Item = &'f Finding>) -> [usize; 3] {
    let mut counts = [0; 3];
    for finding in findings {
        counts[finding.severity as usize] += 1;
    }
    counts
}

struct Paint {
    color: bool,
}

impl Paint {
    fn severity(&self, severity: Severity) -> String {
        let (label, code) = match severity {
            Severity::Error => ("ERROR", "31"),
            Severity::Warning => ("WARN ", "33"),
            Severity::Info => ("INFO ", "36"),
        };
        if self.color {
            format!("\x1b[{code}m{label}\x1b[0m")
        } else {
            label.to_string()
        }
    }

    fn bold(&self, text: &str) -> String {
        if self.color {
            format!("\x1b[1m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }
}

/// The report for people.
pub(crate) fn text(
    report: &Report,
    upgrading: &[ModuleName],
    notices: &[Notice],
    roots: &Roots<'_>,
    color: bool,
) -> String {
    let paint = Paint { color };
    let mut out = String::new();
    let (from, to) = (report.source.series(), report.target.series());
    let _ = writeln!(
        out,
        "{}",
        paint.bold(&format!(
            "Pontix plan {from} -> {to} · {} module(s) checked",
            upgrading.len()
        ))
    );
    let custom: Vec<&Finding> = report
        .findings
        .iter()
        .filter(|finding| in_custom(finding, upgrading))
        .collect();
    let core: Vec<&Finding> = report
        .findings
        .iter()
        .filter(|finding| !in_custom(finding, upgrading))
        .collect();
    let [errors, warnings, infos] = counts(custom.iter().copied());
    let _ = writeln!(
        out,
        "\nCustom modules: {errors} error(s) · {warnings} warning(s) · {infos} info"
    );
    let categories = [
        Category::CodeBreak,
        Category::DataLoss,
        Category::DataRisk,
        Category::Behavior,
        Category::Housekeeping,
    ];
    let by_category: Vec<String> = categories
        .iter()
        .map(|category| {
            let count = custom
                .iter()
                .filter(|finding| {
                    finding.category == *category && finding.severity != Severity::Info
                })
                .count();
            format!("{} {count}", category.as_str().replace('_', " "))
        })
        .collect();
    let _ = writeln!(out, "  {}", by_category.join(" · "));
    let [core_errors, core_warnings, _] = counts(core.iter().copied());
    let _ = writeln!(
        out,
        "Core data: {core_errors} error(s) · {core_warnings} warning(s)"
    );

    for notice in notices {
        let _ = writeln!(out, "\nnote: {}: {}", notice.module, notice.message);
    }

    let mut module: Option<&ModuleName> = None;
    for finding in &custom {
        if module != Some(&finding.module) {
            module = Some(&finding.module);
            let _ = writeln!(out, "\n{}", paint.bold(finding.module.as_str()));
        }
        finding_text(&mut out, finding, roots, &paint);
    }
    if !core.is_empty() {
        let _ = writeln!(
            out,
            "\n{}",
            paint.bold("Core data (every database with these core modules installed)")
        );
        for finding in &core {
            finding_text(&mut out, finding, roots, &paint);
        }
    }

    let not_covered: Vec<String> = report
        .not_covered
        .iter()
        .map(|item| item.replace('_', " "))
        .collect();
    let _ = writeln!(out, "\nNot checked: {}.", not_covered.join(", "));
    let _ = writeln!(
        out,
        "No finding is not the same as safe: test the upgrade on a copy of the database."
    );
    out
}

fn finding_text(out: &mut String, finding: &Finding, roots: &Roots<'_>, paint: &Paint) {
    let _ = writeln!(
        out,
        "  {}  {}  {}",
        paint.severity(finding.severity),
        finding.code,
        finding.message
    );
    let mut shown: Vec<String> = finding
        .locations
        .iter()
        .take(SHOWN_LOCATIONS)
        .map(|location| roots.text(location))
        .collect();
    if finding.locations.len() > SHOWN_LOCATIONS {
        shown.push(format!("(+{})", finding.locations.len() - SHOWN_LOCATIONS));
    }
    let _ = writeln!(out, "         at: {}", shown.join(", "));
    let basis = match &finding.basis {
        Basis::Core(core) => {
            let before = core.before().map(|location| roots.text(location));
            let after = core.after().map_or_else(
                || format!("gone in {}", roots.to.series()),
                |location| roots.text(location),
            );
            match before {
                Some(before) => format!("{before} · {after}"),
                None => after,
            }
        }
        Basis::Rule(rule) => format!("Odoo rule: {rule}"),
        Basis::NotInCore => format!(
            "found in neither {} nor {}: broken before the upgrade, or in a folder Pontix was not given",
            roots.from.series(),
            roots.to.series()
        ),
    };
    let _ = writeln!(out, "         evidence: {basis}");
    if let Some(suggestion) = &finding.suggestion {
        let _ = writeln!(
            out,
            "         suggestion: {} ({})",
            suggestion.rename_to,
            format!("{:?}", suggestion.confidence).to_lowercase()
        );
    }
}

#[derive(Serialize)]
struct JsonReport<'r> {
    schema_version: u32,
    pontix_version: &'static str,
    from: &'static str,
    to: &'static str,
    modules: Vec<&'r str>,
    summary: JsonSummary,
    findings: Vec<JsonFinding<'r>>,
    notices: Vec<JsonNotice<'r>>,
    not_covered: &'static [&'static str],
}

#[derive(Serialize)]
struct JsonSummary {
    error: usize,
    warning: usize,
    info: usize,
}

#[derive(Serialize)]
struct JsonFinding<'r> {
    code: &'static str,
    severity: &'static str,
    category: &'static str,
    module: &'r str,
    scope: &'static str,
    subject: &'r str,
    message: &'r str,
    locations: Vec<JsonLocation>,
    basis: JsonBasis,
    suggestion: Option<JsonSuggestion<'r>>,
}

#[derive(Serialize)]
struct JsonLocation {
    root: RootKind,
    path: String,
    line: u32,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum JsonBasis {
    Core {
        before: Option<JsonLocation>,
        after: Option<JsonLocation>,
    },
    Rule {
        rule: &'static str,
    },
    NotInCore,
}

#[derive(Serialize)]
struct JsonSuggestion<'r> {
    rename_to: &'r str,
    confidence: String,
}

#[derive(Serialize)]
struct JsonNotice<'r> {
    module: &'r str,
    message: &'r str,
}

/// The report for tools and AI agents.
pub(crate) fn json(
    report: &Report,
    upgrading: &[ModuleName],
    notices: &[Notice],
    roots: &Roots<'_>,
) -> String {
    let location = |location: &Location| {
        let (root, path) = roots.place(&location.file);
        JsonLocation {
            root,
            path,
            line: location.line.get(),
        }
    };
    let [error, warning, info] = counts(report.findings.iter());
    let document = JsonReport {
        schema_version: SCHEMA_VERSION,
        pontix_version: pontix_core::VERSION,
        from: report.source.series(),
        to: report.target.series(),
        modules: upgrading.iter().map(ModuleName::as_str).collect(),
        summary: JsonSummary {
            error,
            warning,
            info,
        },
        findings: report
            .findings
            .iter()
            .map(|finding| JsonFinding {
                code: finding.code.as_str(),
                severity: finding.severity.as_str(),
                category: finding.category.as_str(),
                module: finding.module.as_str(),
                scope: if in_custom(finding, upgrading) {
                    "custom"
                } else {
                    "core"
                },
                subject: &finding.subject,
                message: &finding.message,
                locations: finding.locations.iter().map(location).collect(),
                basis: match &finding.basis {
                    Basis::Core(core) => JsonBasis::Core {
                        before: core.before().map(location),
                        after: core.after().map(location),
                    },
                    Basis::Rule(rule) => JsonBasis::Rule { rule },
                    Basis::NotInCore => JsonBasis::NotInCore,
                },
                suggestion: finding
                    .suggestion
                    .as_ref()
                    .map(|suggestion| JsonSuggestion {
                        rename_to: &suggestion.rename_to,
                        confidence: format!("{:?}", suggestion.confidence).to_lowercase(),
                    }),
            })
            .collect(),
        notices: notices
            .iter()
            .map(|notice| JsonNotice {
                module: notice.module.as_str(),
                message: &notice.message,
            })
            .collect(),
        not_covered: report.not_covered,
    };
    // Serializing these plain structs cannot fail.
    let mut text = serde_json::to_string_pretty(&document).unwrap_or_default();
    text.push('\n');
    text
}
