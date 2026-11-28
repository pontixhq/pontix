//! Issues turned into findings: one per module, PTX code and subject, with
//! every place it shows up, a severity and the basis it is reported on.

use std::collections::{BTreeMap, BTreeSet};

use pontix_core::{
    Basis, CodeUse, CoreBasis, Finding, ModuleName, OdooVersion, PtxCode, RefKind, Severity,
    Suggestion,
};

use crate::{ApiUse, Issue, ModelUse, PathUse, Problem, Scan, Since};

/// What Pontix does not check yet. A report always lists them, so that no
/// finding never reads as safe.
pub const NOT_COVERED: &[&str] = &[
    "js",
    // QWeb outside reports (website pages, mail templates, portal), and
    // report variables Pontix cannot type.
    "qweb_outside_reports",
    // Python code whose model Pontix cannot tell, such as a method
    // parameter.
    "untyped_python",
];

/// The findings of a scan, ready for people and tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The version upgraded from.
    pub source: OdooVersion,
    /// The version upgraded to.
    pub target: OdooVersion,
    /// Findings by module, then severity, code and subject.
    pub findings: Vec<Finding>,
    /// What Pontix did not check: [`NOT_COVERED`].
    pub not_covered: &'static [&'static str],
}

/// Groups the issues of a scan into findings.
pub fn report(scan: &Scan) -> Report {
    let target = scan.changes.target.series();
    let mut grouped: BTreeMap<(ModuleName, Severity, PtxCode, String), Finding> = BTreeMap::new();
    let mut demo: BTreeMap<ModuleName, BTreeSet<&str>> = BTreeMap::new();
    for issue in &scan.issues {
        if let Problem::DemoRecord { xmlid } = &issue.problem {
            demo.entry(issue.module.clone()).or_default().insert(xmlid);
        }
        let classified = classify(issue, target);
        let severity = match issue.since {
            Since::Before => Severity::Info,
            Since::Upgrade => classified.severity.unwrap_or(classified.code.severity()),
        };
        let key = (
            issue.module.clone(),
            severity,
            classified.code,
            classified.subject.clone(),
        );
        let finding = grouped.entry(key).or_insert_with(|| Finding {
            code: classified.code,
            severity,
            category: classified.code.category(),
            module: issue.module.clone(),
            subject: classified.subject,
            message: classified.message,
            locations: Vec::new(),
            basis: basis(issue),
            suggestion: classified.suggestion,
        });
        if !finding.locations.contains(&issue.at) {
            finding.locations.push(issue.at.clone());
        }
    }
    let mut findings: Vec<Finding> = grouped.into_values().collect();
    for finding in &mut findings {
        if finding.code == PtxCode::T802
            && let Some(records) = demo.get(&finding.module)
        {
            finding.message = demo_message(records, target);
        }
        finding
            .locations
            .sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    }
    Report {
        source: scan.changes.source,
        target: scan.changes.target,
        findings,
        not_covered: NOT_COVERED,
    }
}

fn basis(issue: &Issue) -> Basis {
    if let Some(rule) = issue.evidence.rule {
        return Basis::Rule(rule);
    }
    CoreBasis::new(issue.evidence.before.clone(), issue.evidence.after.clone())
        .map_or(Basis::NotInCore, Basis::Core)
}

struct Classified {
    code: PtxCode,
    /// A severity other than the code's own.
    severity: Option<Severity>,
    subject: String,
    message: String,
    suggestion: Option<Suggestion>,
}

fn classify(issue: &Issue, target: &str) -> Classified {
    let plain = |code, subject: String, message: String| Classified {
        code,
        severity: None,
        subject,
        message,
        suggestion: None,
    };
    match &issue.problem {
        Problem::DependencyGone {
            dependency,
            renamed,
        } => Classified {
            suggestion: renamed.as_ref().map(|renamed| Suggestion {
                rename_to: renamed.to.to_string(),
                confidence: renamed.confidence,
            }),
            ..plain(
                PtxCode::M001,
                dependency.to_string(),
                format!("Dependency {dependency} cannot be installed in {target}."),
            )
        },
        Problem::ManifestVersion { written } => plain(
            PtxCode::M002,
            issue.module.to_string(),
            format!(
                "Manifest version {written} is not a {target} version: Odoo skips the module without an error."
            ),
        ),
        Problem::StaleMigration { version } => plain(
            PtxCode::M003,
            issue.module.to_string(),
            format!(
                "migrations/{version} is for a version below {target}: its scripts never run again."
            ),
        ),
        Problem::ApiGone { api, how } => Classified {
            severity: matches!(how, ApiUse::Test(_)).then_some(Severity::Warning),
            ..plain(
                PtxCode::A101,
                api.clone(),
                format!("{api} does not exist in {target} ({}).", api_use(how)),
            )
        },
        Problem::ApiDeprecated { api, how } => plain(
            PtxCode::A102,
            api.clone(),
            format!(
                "{api} is deprecated in {target} ({}): it still works, but core warns that it will go.",
                api_use(how)
            ),
        ),
        Problem::ModelNameChanges { class, from, to } => plain(
            PtxCode::M004,
            class.clone(),
            format!("Class {class} defines model {to} in {target}, not {from}."),
        ),
        Problem::ManifestUnreadable => plain(
            PtxCode::M005,
            issue.module.to_string(),
            "The manifest is not one literal expression: Odoo ignores the module.".to_string(),
        ),
        Problem::ManifestFileIgnored => plain(
            PtxCode::M006,
            issue.module.to_string(),
            format!("{target} no longer reads __openerp__.py: the module disappears."),
        ),
        Problem::ImportBroken { import } => plain(
            PtxCode::M007,
            import.clone(),
            format!("Import {import} points to nothing: the module fails to load."),
        ),
        Problem::ModelGone {
            model,
            how,
            renamed,
        } => Classified {
            suggestion: renamed.as_ref().map(|renamed| Suggestion {
                rename_to: renamed.to.to_string(),
                confidence: renamed.confidence,
            }),
            // Broken tests fail CI, not the database.
            severity: (*how == ModelUse::Test).then_some(Severity::Warning),
            ..plain(
                if renamed.is_some() {
                    PtxCode::M102
                } else {
                    PtxCode::M101
                },
                model.to_string(),
                format!(
                    "Model {model} does not exist in {target} ({}).",
                    model_use(how)
                ),
            )
        },
        Problem::ModelNameTaken { model } => plain(
            PtxCode::M103,
            model.to_string(),
            format!("Model {model}, defined by this module, now also exists in {target} core."),
        ),
        Problem::FieldGone {
            model,
            field,
            path,
            how,
            renamed,
        } => Classified {
            suggestion: renamed.as_ref().map(|renamed| Suggestion {
                rename_to: format!("{}.{}", renamed.to.0, renamed.to.1),
                confidence: renamed.confidence,
            }),
            severity: matches!(how, PathUse::Test(_)).then_some(Severity::Warning),
            ..plain(
                if renamed.is_some() {
                    PtxCode::F202
                } else {
                    PtxCode::F201
                },
                format!("{model}.{field}"),
                format!(
                    "Field {model}.{field} does not exist in {target} (path {path} in {}).",
                    path_use(how)
                ),
            )
        },
        Problem::RedeclaredFieldType {
            model,
            field,
            from,
            to,
        } => plain(
            PtxCode::F203,
            format!("{model}.{field}"),
            format!(
                "Core field {model}.{field}, declared again here, changes type from {from} to {to}."
            ),
        ),
        Problem::RedeclaredFieldGone { model, field } => plain(
            PtxCode::F205,
            format!("{model}.{field}"),
            format!(
                "Core field {model}.{field}, declared again here, is gone from {target} core: this declaration now makes a field of its own."
            ),
        ),
        Problem::OverriddenMethodGone {
            model,
            method,
            calls_super,
        } => plain(
            if *calls_super {
                PtxCode::P401
            } else {
                PtxCode::P404
            },
            format!("{model}.{method}"),
            if *calls_super {
                format!(
                    "Overridden method {model}.{method} is gone from {target} core: super() fails."
                )
            } else {
                format!(
                    "Overridden method {model}.{method} is gone from {target} core: nothing calls this override any more."
                )
            },
        ),
        Problem::OverriddenMethodParams {
            model,
            method,
            from,
            to,
        } => Classified {
            severity: only_renamed(from, to).then_some(Severity::Info),
            ..plain(
                PtxCode::P402,
                format!("{model}.{method}"),
                format!(
                    "Overridden method {model}.{method} takes ({}) in {target} core, not ({}).",
                    to.join(", "),
                    from.join(", ")
                ),
            )
        },
        Problem::OverriddenMethodBody {
            model,
            method,
            calls_super,
        } => Classified {
            // Core methods that overrides extend change in most releases:
            // measured on 10 OCA repositories, an override calling super()
            // rarely needed a change, so it is only a hint. An override
            // that replaces core logic may be an outdated copy of it.
            severity: calls_super.then_some(Severity::Info),
            ..plain(
                PtxCode::P403,
                format!("{model}.{method}"),
                if *calls_super {
                    format!(
                        "Core logic this override of {model}.{method} extends changed in {target}: check its behavior."
                    )
                } else {
                    format!(
                        "Core code this override of {model}.{method} replaces changed in {target}: the override may be an outdated copy."
                    )
                },
            )
        },
        Problem::RecordGone { xmlid, how } => Classified {
            severity: (*how == RefKind::Test).then_some(Severity::Warning),
            ..plain(
                PtxCode::R601,
                xmlid.clone(),
                format!(
                    "Record {xmlid} does not exist in {target} ({}).",
                    ref_kind(*how)
                ),
            )
        },
        Problem::FieldParameterGone {
            model,
            field,
            field_type,
            parameter,
        } => plain(
            PtxCode::F206,
            format!("{model}.{field}.{parameter}"),
            format!(
                "Field {model}.{field} is declared with {parameter}=, which {field_type} fields no longer take in {target}: Odoo ignores it, with a warning."
            ),
        ),
        Problem::ViewTargetGone { view, spec, xpath } => plain(
            PtxCode::X301,
            format!("{view} {spec}"),
            format!(
                "{} {spec} finds nothing in view {view} as {target} has it.",
                if *xpath { "XPath" } else { "Locator" }
            ),
        ),
        Problem::DemoRecordInData { xmlid } => plain(
            PtxCode::R602,
            xmlid.clone(),
            format!(
                "Data file refers to record {xmlid}, which only demo data creates: installing on a {target} database without demo data fails."
            ),
        ),
        // One finding per module: the fix is one choice for all its tests.
        // The message, naming the records, is written once they are known.
        Problem::DemoRecord { xmlid } => plain(
            PtxCode::T802,
            issue.module.to_string(),
            demo_message(&BTreeSet::from([xmlid.as_str()]), target),
        ),
        Problem::DataGone {
            model,
            field,
            storage,
            renamed,
        } => {
            let subject = match field {
                Some(field) => format!("{model}.{field}"),
                None => model.to_string(),
            };
            let what = match field {
                Some(_) => format!("Stored field {subject} ({storage:?})"),
                None => format!("The table of model {subject}"),
            };
            Classified {
                suggestion: renamed.as_ref().map(|renamed| Suggestion {
                    rename_to: match &renamed.to.1 {
                        Some(field) => format!("{}.{field}", renamed.to.0),
                        None => renamed.to.0.to_string(),
                    },
                    confidence: renamed.confidence,
                }),
                ..plain(
                    PtxCode::D901,
                    subject,
                    format!(
                        "{what} goes away in {target}, with its data, unless an upgrade script moves it."
                    ),
                )
            }
        }
        Problem::SelectionKeysGone { model, field, keys } => plain(
            PtxCode::D902,
            format!("{model}.{field}"),
            format!(
                "Selection field {model}.{field} no longer accepts {} in {target}: stored rows may hold them.",
                keys.join(", ")
            ),
        ),
        Problem::StoredTypeChanged {
            model,
            field,
            from,
            to,
        } => plain(
            PtxCode::D903,
            format!("{model}.{field}"),
            format!(
                "Stored field {model}.{field} changes type from {from} to {to}: its values must be converted."
            ),
        ),
    }
}

/// Only the names differ: same count and the same `*`/`**` parameters. A
/// positional call keeps working.
fn only_renamed(from: &[String], to: &[String]) -> bool {
    let stars = |name: &String| name.chars().take_while(|c| *c == '*').count();
    from.len() == to.len() && from.iter().zip(to).all(|(a, b)| stars(a) == stars(b))
}

fn model_use(how: &ModelUse) -> String {
    match how {
        ModelUse::Inherit => "inherited".to_string(),
        ModelUse::Delegate => "delegated to".to_string(),
        ModelUse::Comodel(field) => format!("comodel of field {field}"),
        ModelUse::Record => "model of a record in a data file".to_string(),
        ModelUse::Code => "opened through env in Python code".to_string(),
        ModelUse::Test => "opened through env in tests".to_string(),
    }
}

fn path_use(how: &PathUse) -> String {
    match how {
        PathUse::Related(field) => format!("related= of field {field}"),
        PathUse::Depends(method) => format!("@api.depends of {method}"),
        PathUse::Onchange(method) => format!("@api.onchange of {method}"),
        PathUse::Constrains(method) => format!("@api.constrains of {method}"),
        PathUse::View(view) => format!("view {view}"),
        PathUse::ViewExpression(view, attribute) => format!("{attribute}= in view {view}"),
        PathUse::Qweb(template, directive) => format!("{directive}= in template {template}"),
        PathUse::RecordValue(record) => format!("a value of record {record}"),
        PathUse::Code(how) => format!("Python code, {}", code_use(*how)),
        PathUse::Test(how) => format!("tests, {}", code_use(*how)),
    }
}

fn api_use(how: &ApiUse) -> String {
    match how {
        ApiUse::Import => "imported".to_string(),
        ApiUse::Attribute => "used through an imported module".to_string(),
        ApiUse::Call(model) => format!("called on {model} records"),
        ApiUse::Test(inner) => format!("{} in tests", api_use(inner)),
    }
}

fn code_use(how: CodeUse) -> &'static str {
    match how {
        CodeUse::Attribute => "attribute",
        CodeUse::Values => "values written",
        CodeUse::Domain => "domain",
        CodeUse::FieldString => "field name in a string",
        CodeUse::Call => "method call",
        CodeUse::Import => "import",
    }
}

/// The message of a T802 finding: how many demo records the tests use, and
/// the first few.
fn demo_message(records: &BTreeSet<&str>, target: &str) -> String {
    // Four names fit as well as three and an ellipsis.
    const NAMED: usize = 3;
    let mut named: Vec<&str> = records.iter().copied().collect();
    if named.len() > NAMED + 1 {
        named.truncate(NAMED);
        named.push("...");
    }
    let records = match records.len() {
        1 => format!("record {}", named.join(", ")),
        count => format!("{count} records ({})", named.join(", ")),
    };
    format!(
        "Tests use {records} that only demo data creates: {target} makes databases without demo data unless run with --with-demo."
    )
}

fn ref_kind(how: RefKind) -> &'static str {
    match how {
        RefKind::Ref => "ref=",
        RefKind::Eval => "ref() in eval",
        RefKind::Groups => "groups=",
        RefKind::Action => "action",
        RefKind::Parent => "menu parent",
        RefKind::InheritId => "inherited view",
        RefKind::Python => "env.ref() in Python",
        RefKind::Test => "env.ref() in tests",
    }
}
