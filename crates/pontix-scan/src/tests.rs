use std::num::NonZeroU32;
use std::path::PathBuf;

use pontix_core::{
    ApiDecorator, CodeRef, CodeRoot, CodeUse, FieldAttrs, FieldDef, FieldPaths, FieldType, Inherit,
    Manifest, MethodDef, ModelBase, ModelDef, OdooVersion, RefKind, Severity, Storage, Written,
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
        code_refs: Vec::new(),
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
        xmlid_refs: Vec::new(),
        code_refs: Vec::new(),
        test_refs: Vec::new(),
        data: Vec::new(),
    }
}

fn addons(version: OdooVersion, modules: Vec<Module>) -> Addons {
    Addons {
        version,
        modules,
        diagnostics: Vec::new(),
        framework: Default::default(),
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

/// What a scan finds in the custom code itself: not its manifest version,
/// and not what core does to data.
fn in_code(issues: Vec<Issue>) -> Vec<Issue> {
    issues
        .into_iter()
        .filter(|issue| issue.module.as_str().starts_with("custom"))
        .filter(|issue| !matches!(issue.problem, Problem::ManifestVersion { .. }))
        .collect()
}

#[test]
fn finds_each_kind_of_breakage_with_its_evidence() {
    let found: Vec<(u32, Problem, Since)> = in_code(scanned().issues)
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
            rule: None,
        }
    );
    assert_eq!(
        evidence(20),
        Evidence {
            before: Some(at("sale/models.py", 20)),
            after: Some(at("sale/models.py", 20)),
            rule: None,
        }
    );
    assert_eq!(
        evidence(1),
        Evidence {
            before: Some(at("sale_legacy/__manifest__.py", 1)),
            after: None,
            rule: None,
        }
    );
}

#[test]
fn an_unchanged_core_gives_no_issues() {
    let mut target = source();
    target.version = OdooVersion::V19;
    let issues: Vec<Issue> =
        in_code(scan(&source(), &target, &[ModuleName::new("custom_sale")], &[]).issues)
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
    let problems: Vec<Problem> =
        in_code(scan(&source, &target, &[ModuleName::new("custom_list")], &[]).issues)
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

fn data(
    file: &str,
    records: Vec<pontix_core::Record>,
    refs: Vec<pontix_core::XmlRef>,
) -> (PathBuf, pontix_core::DataFile) {
    (
        PathBuf::from(file),
        pontix_core::DataFile {
            records,
            models: Vec::new(),
            refs,
        },
    )
}

fn view(
    file: &str,
    id: &str,
    model: Option<&str>,
    inherit: Option<&str>,
    fields: &[(&str, u32)],
    line: u32,
) -> pontix_core::Record {
    pontix_core::Record {
        id: Some(id.to_string()),
        model: name("ir.ui.view"),
        fields: Vec::new(),
        view: Some(pontix_core::View {
            model: model.map(ModelName::new),
            inherit_id: inherit.map(str::to_string),
            fields: fields
                .iter()
                .map(|(field, at_line)| pontix_core::ArchField {
                    name: FieldName::new(*field),
                    within: Vec::new(),
                    locator: false,
                    location: at(file, *at_line),
                })
                .collect(),
        }),
        location: at(file, line),
    }
}

fn reference(file: &str, xmlid: &str, kind: RefKind, line: u32) -> pontix_core::XmlRef {
    pontix_core::XmlRef {
        xmlid: xmlid.to_string(),
        kind,
        location: at(file, line),
    }
}

#[test]
fn checks_views_records_and_external_ids_in_data_files() {
    const CORE: &str = "sale/views/sale.xml";
    const MINE: &str = "custom_sale/views/sale.xml";
    let mut source = source();
    let mut target = target();
    let core_view = || {
        view(
            CORE,
            "view_order_form",
            Some("sale.order"),
            None,
            &[("note", 5)],
            3,
        )
    };
    let old_action = pontix_core::Record {
        id: Some("action_old".to_string()),
        model: name("ir.actions.act_window"),
        fields: Vec::new(),
        view: None,
        location: at(CORE, 20),
    };
    let records = || {
        [
            model(
                "base/models.py",
                "ir.ui.view",
                Inherit::Absent,
                vec![],
                vec![],
            ),
            model(
                "base/models.py",
                "ir.actions.act_window",
                Inherit::Absent,
                vec![],
                vec![],
            ),
        ]
    };
    source.modules[0].models.extend(records());
    target.modules[0].models.extend(records());
    source.modules[1].data = vec![data(CORE, vec![core_view(), old_action], vec![])];
    target.modules[1].data = vec![data(CORE, vec![core_view()], vec![])];
    let mine = data(
        MINE,
        vec![
            view(
                MINE,
                "view_order_form_custom",
                None,
                Some("sale.view_order_form"),
                &[("partner_id", 6), ("note", 7)],
                2,
            ),
            pontix_core::Record {
                id: Some("legacy_demo".to_string()),
                model: name("sale.legacy"),
                fields: Vec::new(),
                view: None,
                location: at(MINE, 10),
            },
        ],
        vec![
            reference(MINE, "sale.view_order_form", RefKind::InheritId, 4),
            reference(MINE, "sale.action_old", RefKind::Action, 12),
            reference(MINE, "sale.model_sale_order", RefKind::Ref, 13),
            reference(MINE, "view_order_form_custom", RefKind::Ref, 14),
        ],
    );
    for addons in [&mut source, &mut target] {
        let custom = addons
            .modules
            .iter_mut()
            .find(|module| module.name.as_str() == "custom_sale")
            .expect("the custom module");
        custom.data = vec![mine.clone()];
        custom.xmlid_refs = vec![reference(
            "custom_sale/models.py",
            "sale.action_old",
            RefKind::Python,
            70,
        )];
    }
    let found: Vec<(String, u32, Problem)> =
        scan(&source, &target, &[ModuleName::new("custom_sale")], &[])
            .issues
            .into_iter()
            .filter(|issue| {
                issue.at.file.extension().is_some_and(|ext| ext == "xml")
                    || issue.at.line.get() == 70
            })
            .map(|issue| {
                (
                    issue.at.file.display().to_string(),
                    issue.at.line.get(),
                    issue.problem,
                )
            })
            .collect();
    let gone = |file: &str, line| {
        (
            file.to_string(),
            line,
            Problem::RecordGone {
                xmlid: "sale.action_old".to_string(),
                how: if line == 70 {
                    RefKind::Python
                } else {
                    RefKind::Action
                },
            },
        )
    };
    assert_eq!(
        found,
        [
            gone("custom_sale/models.py", 70),
            (
                MINE.to_string(),
                7,
                Problem::FieldGone {
                    model: name("sale.order"),
                    field: FieldName::new("note"),
                    path: "note".to_string(),
                    how: PathUse::View("custom_sale.view_order_form_custom".to_string()),
                    renamed: None,
                },
            ),
            (
                MINE.to_string(),
                10,
                Problem::ModelGone {
                    model: name("sale.legacy"),
                    how: ModelUse::Record,
                    renamed: None,
                },
            ),
            gone(MINE, 12),
        ]
    );
}

#[test]
fn reports_manifest_versions_and_what_core_does_to_data() {
    let issues = scanned().issues;
    let manifest: Vec<&Problem> = issues
        .iter()
        .filter(|issue| issue.module.as_str() == "custom_sale" && issue.at.line.get() == 1)
        .map(|issue| &issue.problem)
        .collect();
    assert!(manifest.contains(&&Problem::ManifestVersion {
        written: "18.0.1.0.0".to_string()
    }));
    let data: Vec<(&str, &Problem)> = issues
        .iter()
        .filter(|issue| !issue.module.as_str().starts_with("custom"))
        .map(|issue| (issue.module.as_str(), &issue.problem))
        .collect();
    let gone = |model: &str, field: Option<&str>| Problem::DataGone {
        model: name(model),
        field: field.map(FieldName::new),
        storage: Storage::Column,
        renamed: None,
    };
    assert_eq!(
        data,
        [
            ("base", &gone("res.partner", Some("fax"))),
            ("sale", &gone("sale.legacy", None)),
            (
                "sale",
                &Problem::StoredTypeChanged {
                    model: name("sale.order"),
                    field: FieldName::new("amount"),
                    from: FieldType::Monetary,
                    to: FieldType::Float,
                }
            ),
            ("sale", &gone("sale.order", Some("note"))),
            ("sale", &gone("sale.order", Some("legacy_state"))),
        ]
    );
}

#[test]
fn findings_group_issues_with_a_code_severity_and_basis() {
    use pontix_core::{Basis, PtxCode, Severity};

    let report = report(&scanned());
    assert_eq!(report.not_covered, NOT_COVERED);
    let mine: Vec<(PtxCode, Severity, &str, usize)> = report
        .findings
        .iter()
        .filter(|finding| finding.module.as_str() == "custom_sale")
        .map(|finding| {
            (
                finding.code,
                finding.severity,
                finding.subject.as_str(),
                finding.locations.len(),
            )
        })
        .collect();
    assert_eq!(
        mine,
        [
            (PtxCode::M001, Severity::Error, "sale_legacy", 1),
            (PtxCode::M002, Severity::Error, "custom_sale", 1),
            // Used twice: as a comodel and as an inherited model.
            (PtxCode::M101, Severity::Error, "sale.legacy", 2),
            (PtxCode::M103, Severity::Error, "custom.thing", 1),
            (PtxCode::F201, Severity::Error, "res.partner.fax", 1),
            (PtxCode::F201, Severity::Error, "sale.order.note", 1),
            (PtxCode::F203, Severity::Error, "sale.order.amount", 1),
            (PtxCode::P401, Severity::Error, "sale.order._old_hook", 1),
            (
                PtxCode::F205,
                Severity::Warning,
                "sale.order.legacy_state",
                1
            ),
            (
                PtxCode::P402,
                Severity::Warning,
                "sale.order.action_confirm",
                1
            ),
            (
                PtxCode::P403,
                Severity::Warning,
                "sale.order._prepare_invoice",
                1
            ),
            (PtxCode::F201, Severity::Info, "sale.order.ghost", 1),
        ]
    );
    let manifest = report
        .findings
        .iter()
        .find(|finding| finding.code == PtxCode::M002)
        .expect("the manifest finding");
    assert!(matches!(manifest.basis, Basis::Rule(_)));
    let ghost = report
        .findings
        .iter()
        .find(|finding| finding.subject == "sale.order.ghost")
        .expect("the finding broken before the upgrade");
    assert_eq!(ghost.basis, Basis::NotInCore);
    let data = report
        .findings
        .iter()
        .find(|finding| finding.code == PtxCode::D901 && finding.subject == "sale.order.note")
        .expect("the data finding");
    assert_eq!(data.module.as_str(), "sale");
    assert!(matches!(&data.basis, Basis::Core(core) if core.after().is_none()));
}

#[test]
fn only_renamed_parameters_are_info() {
    use pontix_core::{PtxCode, Severity};

    let mut target = target();
    let sale = &mut target.modules[1].models[0];
    sale.methods[0].params = vec!["force".to_string()];
    let mut source = source();
    source.modules[1].models[0].methods[0].params = vec!["forced".to_string()];
    let report = report(&scan(
        &source,
        &target,
        &[ModuleName::new("custom_sale")],
        &[],
    ));
    let params = report
        .findings
        .iter()
        .find(|finding| finding.code == PtxCode::P402)
        .expect("the parameters finding");
    assert_eq!(params.severity, Severity::Info);
}

fn code_ref(root: CodeRoot, path: &[&str], how: CodeUse, line: u32) -> CodeRef {
    CodeRef {
        root,
        path: path.iter().map(|segment| segment.to_string()).collect(),
        how,
        location: at("custom_sale/code.py", line),
    }
}

/// Adds the same Python code to the module being upgraded in `addons`.
fn with_code(mut addons: Addons) -> Addons {
    let custom = addons
        .modules
        .iter_mut()
        .find(|module| module.name.as_str() == "custom_sale")
        .expect("the custom module");
    let sale_order = &mut custom.models[0];
    sale_order.methods[0].code_refs = vec![
        code_ref(
            CodeRoot::Own,
            &["partner_id", "fax"],
            CodeUse::Attribute,
            70,
        ),
        // Not a field before either: a Python attribute Pontix cannot see.
        code_ref(CodeRoot::Own, &["helper", "fax"], CodeUse::Attribute, 71),
        // The module's own field is there as long as its class applies.
        code_ref(CodeRoot::Own, &["legacy_id"], CodeUse::Attribute, 72),
    ];
    custom.code_refs = vec![
        code_ref(
            CodeRoot::Model(name("sale.legacy")),
            &[],
            CodeUse::Attribute,
            80,
        ),
        // A model that never existed is most likely an optional module.
        code_ref(
            CodeRoot::Model(name("never.there")),
            &[],
            CodeUse::Attribute,
            81,
        ),
        code_ref(
            CodeRoot::Model(name("sale.order")),
            &["note"],
            CodeUse::Domain,
            82,
        ),
    ];
    custom.test_refs = vec![code_ref(
        CodeRoot::Model(name("sale.order")),
        &["partner_id", "fax"],
        CodeUse::Values,
        90,
    )];
    addons
}

#[test]
fn code_and_tests_report_only_what_existed_before() {
    let scan = scan(
        &with_code(source()),
        &with_code(target()),
        &[ModuleName::new("custom_sale")],
        &[],
    );
    let found: Vec<(u32, Problem)> = in_code(scan.issues.clone())
        .into_iter()
        .filter(|issue| issue.at.file.ends_with("code.py"))
        .map(|issue| (issue.at.line.get(), issue.problem))
        .collect();
    let gone = |field: &str, path: &str, how: PathUse, model: &str| Problem::FieldGone {
        model: name(model),
        field: FieldName::new(field),
        path: path.to_string(),
        how,
        renamed: None,
    };
    assert_eq!(
        found,
        [
            (
                70,
                gone(
                    "fax",
                    "partner_id.fax",
                    PathUse::Code(CodeUse::Attribute),
                    "res.partner"
                )
            ),
            (
                80,
                Problem::ModelGone {
                    model: name("sale.legacy"),
                    how: ModelUse::Code,
                    renamed: None,
                }
            ),
            (
                82,
                gone("note", "note", PathUse::Code(CodeUse::Domain), "sale.order")
            ),
            (
                90,
                gone(
                    "fax",
                    "partner_id.fax",
                    PathUse::Test(CodeUse::Values),
                    "res.partner"
                )
            ),
        ]
    );
    let severities: Vec<(String, Severity)> = report(&scan)
        .findings
        .into_iter()
        .filter(|finding| {
            finding
                .locations
                .iter()
                .any(|at| at.file.ends_with("code.py"))
        })
        .map(|finding| (finding.message, finding.severity))
        .collect();
    assert!(severities.contains(&(
        "Field res.partner.fax does not exist in 19.0 (path partner_id.fax in tests, values written)."
            .to_string(),
        Severity::Warning
    )));
    assert!(severities.contains(&(
        "Field res.partner.fax does not exist in 19.0 (path partner_id.fax in Python code, attribute)."
            .to_string(),
        Severity::Error
    )));
}

/// A framework tree of `files`, read as [`pontix_index::framework`] does.
fn framework_of(files: &[(&str, &str)]) -> (tempfile::TempDir, pontix_index::Framework) {
    let dir = tempfile::tempdir().expect("a temporary directory");
    for (relative, content) in files {
        let path = dir.path().join(relative);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("directories");
        std::fs::write(path, content).expect("a file");
    }
    let framework = pontix_index::framework(dir.path());
    (dir, framework)
}

#[test]
fn framework_names_and_model_methods_report_what_changed() {
    let (_old, old) = framework_of(&[
        ("odoo/__init__.py", ""),
        (
            "odoo/fields.py",
            "def first(records):\n    return records[:1]\n",
        ),
        (
            "odoo/tools/__init__.py",
            "class ormcache_context:\n    pass\n",
        ),
        (
            "odoo/models.py",
            "class BaseModel:\n    def read_group(self): pass\n    def _where_calc(self): pass\n    def search(self): pass\n",
        ),
    ]);
    let (_new, new) = framework_of(&[
        (
            "odoo/fields/__init__.py",
            "from odoo.orm.domains import Domain\n",
        ),
        ("odoo/orm/domains.py", "class Domain: pass\n"),
        (
            "odoo/tools/__init__.py",
            "import warnings\nclass ormcache_context:\n    def __init__(self):\n        warnings.warn('x', DeprecationWarning)\n",
        ),
        (
            "odoo/orm/models.py",
            "class BaseModel:\n    def read_group(self):\n        \"\"\"Deprecated - use _read_group\"\"\"\n    def search(self): pass\n",
        ),
    ]);
    let code = |mut addons: Addons, framework: pontix_index::Framework| {
        addons.framework = framework;
        let custom = addons
            .modules
            .iter_mut()
            .find(|module| module.name.as_str() == "custom_sale")
            .expect("the custom module");
        custom.models[0].methods[0].code_refs = vec![
            code_ref(CodeRoot::Own, &["read_group"], CodeUse::Call, 70),
            code_ref(
                CodeRoot::Own,
                &["partner_id", "_where_calc"],
                CodeUse::Call,
                71,
            ),
            code_ref(CodeRoot::Own, &["search"], CodeUse::Call, 72),
            // Neither a model method nor a BaseModel one before.
            code_ref(CodeRoot::Own, &["helper"], CodeUse::Call, 73),
        ];
        custom.code_refs = vec![
            code_ref(
                CodeRoot::Framework("odoo.tools.ormcache_context".to_string()),
                &[],
                CodeUse::Import,
                80,
            ),
            code_ref(
                CodeRoot::Framework("odoo.fields.first".to_string()),
                &[],
                CodeUse::Attribute,
                81,
            ),
        ];
        custom.test_refs = vec![code_ref(
            CodeRoot::Framework("odoo.fields.first".to_string()),
            &[],
            CodeUse::Import,
            90,
        )];
        addons
    };
    let scan = scan(
        &code(source(), old),
        &code(target(), new),
        &[ModuleName::new("custom_sale")],
        &[],
    );
    let found: Vec<(u32, Problem)> = in_code(scan.issues.clone())
        .into_iter()
        .filter(|issue| issue.at.file.ends_with("code.py"))
        .map(|issue| (issue.at.line.get(), issue.problem))
        .collect();
    let api = |name: &str| name.to_string();
    assert_eq!(
        found,
        [
            (
                70,
                Problem::ApiDeprecated {
                    api: api("BaseModel.read_group"),
                    how: ApiUse::Call(name("sale.order")),
                }
            ),
            (
                71,
                Problem::ApiGone {
                    api: api("BaseModel._where_calc"),
                    how: ApiUse::Call(name("res.partner")),
                }
            ),
            (
                80,
                Problem::ApiDeprecated {
                    api: api("odoo.tools.ormcache_context"),
                    how: ApiUse::Import,
                }
            ),
            (
                81,
                Problem::ApiGone {
                    api: api("odoo.fields.first"),
                    how: ApiUse::Attribute,
                }
            ),
            (
                90,
                Problem::ApiGone {
                    api: api("odoo.fields.first"),
                    how: ApiUse::Test(Box::new(ApiUse::Import)),
                }
            ),
        ]
    );
    let findings = report(&scan).findings;
    let first: Vec<Severity> = findings
        .iter()
        .filter(|finding| finding.subject == "odoo.fields.first")
        .map(|finding| finding.severity)
        .collect();
    assert_eq!(first, [Severity::Error, Severity::Warning]);
}
