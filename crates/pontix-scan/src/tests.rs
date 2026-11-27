use std::num::NonZeroU32;
use std::path::PathBuf;

use pontix_core::{
    ApiDecorator, FieldAttrs, FieldDef, FieldPaths, FieldType, Inherit, Manifest, MethodDef,
    ModelBase, ModelDef, OdooVersion, Written,
};
use pontix_index::Module;

use super::*;

fn at(file: &str, line: u32) -> Location {
    Location::new(file, NonZeroU32::new(line).expect("lines start at 1"))
}

fn name(value: &str) -> ModelName {
    ModelName::new(value)
}

fn field(
    file: &str,
    field: &str,
    field_type: FieldType,
    comodel: Option<&str>,
    line: u32,
) -> FieldDef {
    FieldDef {
        name: FieldName::new(field),
        field_type,
        comodel: comodel.map(ModelName::new),
        attrs: FieldAttrs::default(),
        location: at(file, line),
    }
}

fn related(file: &str, field_name: &str, path: &str, line: u32) -> FieldDef {
    FieldDef {
        attrs: FieldAttrs {
            related: Written::Value(path.to_string()),
            ..FieldAttrs::default()
        },
        ..field(file, field_name, FieldType::Char, None, line)
    }
}

fn method(file: &str, name: &str, params: &[&str], fingerprint: u64, line: u32) -> MethodDef {
    MethodDef {
        name: name.to_string(),
        params: params.iter().map(|p| p.to_string()).collect(),
        calls_super: false,
        field_paths: Vec::new(),
        fingerprint,
        location: at(file, line),
    }
}

fn overriding(file: &str, name: &str, line: u32) -> MethodDef {
    MethodDef {
        calls_super: true,
        ..method(file, name, &[], 1000 + u64::from(line), line)
    }
}

fn model(
    file: &str,
    model: &str,
    inherit: Inherit,
    fields: Vec<FieldDef>,
    methods: Vec<MethodDef>,
) -> ModelDef {
    ModelDef {
        class_name: "Class".to_string(),
        name: (inherit == Inherit::Absent).then(|| ModelName::new(model)),
        inherit,
        inherits: Vec::new(),
        base: ModelBase::Model,
        auto: Written::Absent,
        table_query: false,
        fields,
        methods,
        location: at(file, 1),
    }
}

fn extends(model: &str) -> Inherit {
    Inherit::Single(ModelName::new(model))
}

fn module(name: &str, depends: &[&str], version: Option<&str>, models: Vec<ModelDef>) -> Module {
    Module {
        name: ModuleName::new(name),
        path: PathBuf::from(name),
        manifest_path: PathBuf::from(format!("{name}/__manifest__.py")),
        manifest: Manifest {
            depends: depends.iter().copied().map(ModuleName::new).collect(),
            version: version.map(str::to_string),
            ..Manifest::default()
        },
        python_files: Vec::new(),
        models,
    }
}

fn addons(version: OdooVersion, modules: Vec<Module>) -> Addons {
    Addons {
        version,
        modules,
        diagnostics: Vec::new(),
    }
}

/// The module being upgraded, the same in both versions.
fn custom() -> Module {
    const FILE: &str = "custom_sale/models.py";
    let mut compute = method(FILE, "_compute_summary", &[], 7, 40);
    compute.field_paths = vec![FieldPaths {
        decorator: ApiDecorator::Depends,
        paths: vec![
            "note".to_string(),
            "partner_id.name".to_string(),
            "ghost".to_string(),
        ],
        location: at(FILE, 39),
    }];
    module(
        "custom_sale",
        &["sale", "sale_legacy"],
        Some("18.0.1.0.0"),
        vec![
            model(
                FILE,
                "sale.order",
                extends("sale.order"),
                vec![
                    field(FILE, "amount", FieldType::Monetary, None, 10),
                    field(FILE, "legacy_state", FieldType::Char, None, 11),
                    related(FILE, "partner_fax", "partner_id.fax", 12),
                    field(
                        FILE,
                        "legacy_id",
                        FieldType::Many2one,
                        Some("sale.legacy"),
                        13,
                    ),
                ],
                vec![
                    overriding(FILE, "action_confirm", 20),
                    overriding(FILE, "_prepare_invoice", 25),
                    overriding(FILE, "_old_hook", 30),
                    compute,
                ],
            ),
            ModelDef {
                location: at(FILE, 50),
                ..model(FILE, "custom.thing", Inherit::Absent, vec![], vec![])
            },
            ModelDef {
                location: at(FILE, 60),
                ..model(FILE, "sale.legacy", extends("sale.legacy"), vec![], vec![])
            },
        ],
    )
}

fn source() -> Addons {
    const FILE: &str = "sale/models.py";
    addons(
        OdooVersion::V18,
        vec![
            module(
                "base",
                &[],
                None,
                vec![model(
                    "base/models.py",
                    "res.partner",
                    Inherit::Absent,
                    vec![
                        field("base/models.py", "name", FieldType::Char, None, 3),
                        field("base/models.py", "fax", FieldType::Char, None, 4),
                    ],
                    vec![],
                )],
            ),
            module(
                "sale",
                &["base"],
                None,
                vec![
                    model(
                        FILE,
                        "sale.order",
                        Inherit::Absent,
                        vec![
                            field(
                                FILE,
                                "partner_id",
                                FieldType::Many2one,
                                Some("res.partner"),
                                5,
                            ),
                            field(FILE, "amount", FieldType::Monetary, None, 6),
                            field(FILE, "note", FieldType::Text, None, 7),
                            field(FILE, "legacy_state", FieldType::Char, None, 8),
                        ],
                        vec![
                            method(FILE, "action_confirm", &[], 1, 20),
                            method(FILE, "_prepare_invoice", &[], 2, 25),
                            method(FILE, "_old_hook", &[], 3, 30),
                        ],
                    ),
                    model(FILE, "sale.legacy", Inherit::Absent, vec![], vec![]),
                ],
            ),
            module("sale_legacy", &["sale"], None, vec![]),
            custom(),
        ],
    )
}

fn target() -> Addons {
    const FILE: &str = "sale/models.py";
    addons(
        OdooVersion::V19,
        vec![
            module(
                "base",
                &[],
                None,
                vec![model(
                    "base/models.py",
                    "res.partner",
                    Inherit::Absent,
                    vec![field("base/models.py", "name", FieldType::Char, None, 3)],
                    vec![],
                )],
            ),
            module(
                "sale",
                &["base"],
                None,
                vec![
                    model(
                        FILE,
                        "sale.order",
                        Inherit::Absent,
                        vec![
                            field(
                                FILE,
                                "partner_id",
                                FieldType::Many2one,
                                Some("res.partner"),
                                5,
                            ),
                            field(FILE, "amount", FieldType::Float, None, 6),
                        ],
                        vec![
                            method(FILE, "action_confirm", &["force"], 1, 20),
                            method(FILE, "_prepare_invoice", &[], 99, 25),
                        ],
                    ),
                    model(FILE, "custom.thing", Inherit::Absent, vec![], vec![]),
                ],
            ),
            custom(),
        ],
    )
}

fn scanned() -> Scan {
    scan(&source(), &target(), &[ModuleName::new("custom_sale")], &[])
}

#[test]
fn finds_each_kind_of_breakage_with_its_evidence() {
    let found: Vec<(u32, Problem, Since)> = scanned()
        .issues
        .into_iter()
        .map(|issue| (issue.at.line.get(), issue.problem, issue.since))
        .collect();
    let gone = |field: &str, path: &str, how: PathUse, model: &str| Problem::FieldGone {
        model: name(model),
        field: FieldName::new(field),
        path: path.to_string(),
        how,
        renamed: None,
    };
    let depends = || PathUse::Depends("_compute_summary".to_string());
    assert_eq!(
        found,
        [
            (
                1,
                Problem::DependencyGone {
                    dependency: ModuleName::new("sale_legacy"),
                    renamed: None,
                },
                Since::Upgrade,
            ),
            (
                10,
                Problem::RedeclaredFieldType {
                    model: name("sale.order"),
                    field: FieldName::new("amount"),
                    from: FieldType::Monetary,
                    to: FieldType::Float,
                },
                Since::Upgrade,
            ),
            (
                11,
                Problem::RedeclaredFieldGone {
                    model: name("sale.order"),
                    field: FieldName::new("legacy_state"),
                },
                Since::Upgrade,
            ),
            (
                12,
                gone(
                    "fax",
                    "partner_id.fax",
                    PathUse::Related(FieldName::new("partner_fax")),
                    "res.partner"
                ),
                Since::Upgrade,
            ),
            (
                13,
                Problem::ModelGone {
                    model: name("sale.legacy"),
                    how: ModelUse::Comodel(FieldName::new("legacy_id")),
                    renamed: None,
                },
                Since::Upgrade,
            ),
            (
                20,
                Problem::OverriddenMethodParams {
                    model: name("sale.order"),
                    method: "action_confirm".to_string(),
                    from: vec![],
                    to: vec!["force".to_string()],
                },
                Since::Upgrade,
            ),
            (
                25,
                Problem::OverriddenMethodBody {
                    model: name("sale.order"),
                    method: "_prepare_invoice".to_string(),
                    calls_super: true,
                },
                Since::Upgrade,
            ),
            (
                30,
                Problem::OverriddenMethodGone {
                    model: name("sale.order"),
                    method: "_old_hook".to_string(),
                    calls_super: true,
                },
                Since::Upgrade,
            ),
            (
                39,
                gone("note", "note", depends(), "sale.order"),
                Since::Upgrade
            ),
            // Missing in both versions: not the upgrade's doing.
            (
                39,
                gone("ghost", "ghost", depends(), "sale.order"),
                Since::Before
            ),
            (
                50,
                Problem::ModelNameTaken {
                    model: name("custom.thing"),
                },
                Since::Upgrade,
            ),
            (
                60,
                Problem::ModelGone {
                    model: name("sale.legacy"),
                    how: ModelUse::Inherit,
                    renamed: None,
                },
                Since::Upgrade,
            ),
        ]
    );
}

#[test]
fn evidence_points_at_core_in_each_version() {
    let issues = scanned().issues;
    let evidence = |line: u32| {
        issues
            .iter()
            .find(|issue| issue.at.line.get() == line)
            .map(|issue| issue.evidence.clone())
            .expect("an issue on that line")
    };
    assert_eq!(
        evidence(12),
        Evidence {
            before: Some(at("base/models.py", 4)),
            after: None,
        }
    );
    assert_eq!(
        evidence(20),
        Evidence {
            before: Some(at("sale/models.py", 20)),
            after: Some(at("sale/models.py", 20)),
        }
    );
    assert_eq!(
        evidence(1),
        Evidence {
            before: Some(at("sale_legacy/__manifest__.py", 1)),
            after: None,
        }
    );
}

#[test]
fn an_unchanged_core_gives_no_issues() {
    let mut target = source();
    target.version = OdooVersion::V19;
    let issues: Vec<Issue> = scan(&source(), &target, &[ModuleName::new("custom_sale")], &[])
        .issues
        .into_iter()
        .filter(|issue| issue.since == Since::Upgrade)
        .collect();
    assert_eq!(issues, []);
}

#[test]
fn a_class_that_names_another_model_in_the_target_is_reported() {
    let class = ModelDef {
        class_name: "SaleOrderCustom".to_string(),
        inherit: Inherit::List(vec![name("sale.order")]),
        ..model(
            "custom/models.py",
            "sale.order",
            extends("sale.order"),
            vec![],
            vec![],
        )
    };
    let mut source = source();
    let mut target = target();
    for addons in [&mut source, &mut target] {
        addons
            .modules
            .push(module("custom_list", &["sale"], None, vec![class.clone()]));
    }
    let problems: Vec<Problem> = scan(&source, &target, &[ModuleName::new("custom_list")], &[])
        .issues
        .into_iter()
        .map(|issue| issue.problem)
        .collect();
    assert_eq!(
        problems,
        // In 18 a one-item `_inherit` list names the model; in 19 only a
        // string does, and the class name gives the name instead.
        [Problem::ModelNameChanges {
            class: "SaleOrderCustom".to_string(),
            from: name("sale.order"),
            to: name("sale.order.custom"),
        }]
    );
}
