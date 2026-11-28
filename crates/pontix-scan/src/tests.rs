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
        params: Vec::new(),
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
            xmlids: Vec::new(),
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
            ..pontix_core::View::default()
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
            (PtxCode::F201, Severity::Info, "sale.order.ghost", 1),
            // An override calling super() is only a hint.
            (
                PtxCode::P403,
                Severity::Info,
                "sale.order._prepare_invoice",
                1
            ),
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

#[test]
fn a_field_only_another_module_declares_is_not_there_for_this_one() {
    // Another module being upgraded declares `fax` again, but custom_sale
    // does not depend on it: for custom_sale the field is still gone.
    let with_other = |mut addons: Addons| {
        addons.modules.push(module(
            "partner_fax",
            &["base"],
            Some("18.0.1.0.0"),
            vec![model(
                "partner_fax/models.py",
                "res.partner",
                extends("res.partner"),
                vec![field(
                    "partner_fax/models.py",
                    "fax",
                    FieldType::Char,
                    None,
                    5,
                )],
                vec![],
            )],
        ));
        addons
    };
    let upgrading = [
        ModuleName::new("custom_sale"),
        ModuleName::new("partner_fax"),
    ];
    let scan = scan(
        &with_other(source()),
        &with_other(target()),
        &upgrading,
        &[],
    );
    let fax: Vec<(&str, &Problem)> = scan
        .issues
        .iter()
        .filter(|issue| matches!(&issue.problem, Problem::FieldGone { field, .. } if field.as_str() == "fax"))
        .map(|issue| (issue.module.as_str(), &issue.problem))
        .collect();
    assert_eq!(fax.len(), 1, "{fax:?}");
    assert_eq!(fax[0].0, "custom_sale");
}

#[test]
fn tests_that_need_gone_or_demo_records_are_warnings() {
    use pontix_core::PtxCode;

    const DATA: &str = "sale/data/sale_data.xml";
    const DEMO: &str = "sale/data/sale_demo.xml";
    const MINE: &str = "custom_sale/demo/demo.xml";
    const TEST: &str = "custom_sale/tests/test_sale.py";
    let record = |file: &str, id: &str, line| pontix_core::Record {
        id: Some(id.to_string()),
        model: name("sale.order"),
        fields: Vec::new(),
        view: None,
        location: at(file, line),
    };
    let sale = |module: &mut Module, gone: bool| {
        module.manifest.demo = vec!["data/sale_demo.xml".to_string()];
        let mut data_file = data(DATA, vec![], vec![]);
        if gone {
            data_file.1.records.push(record(DATA, "action_old", 3));
        }
        // A record that Python makes and `_update_xmlids` names, which demo
        // data then updates: it is there without demo data.
        data_file.1.xmlids = vec![("sale.warehouse_order".to_string(), at(DATA, 5))];
        let demo = data(
            DEMO,
            vec![
                record(DEMO, "order_demo", 2),
                record(DEMO, "warehouse_order", 4),
            ],
            vec![],
        );
        module.data = vec![data_file, demo];
    };
    let (mut source, mut target) = (source(), target());
    sale(&mut source.modules[1], true);
    sale(&mut target.modules[1], false);
    for addons in [&mut source, &mut target] {
        let custom = addons
            .modules
            .iter_mut()
            .find(|module| module.name.as_str() == "custom_sale")
            .expect("the custom module");
        custom.manifest.demo = vec!["demo/demo.xml".to_string()];
        custom.data = vec![data(MINE, vec![record(MINE, "custom_demo", 2)], vec![])];
        custom.xmlid_refs = [
            ("sale.action_old", 10),
            ("sale.order_demo", 11),
            ("sale.warehouse_order", 12),
            ("custom_demo", 13),
            ("sale.never_seen", 14),
            ("sale.order_demo", 15),
        ]
        .into_iter()
        .map(|(xmlid, line)| reference(TEST, xmlid, RefKind::Test, line))
        .collect();
    }
    let scan = scan(&source, &target, &[ModuleName::new("custom_sale")], &[]);
    let found: Vec<(u32, &Problem)> = scan
        .issues
        .iter()
        .filter(|issue| issue.at.file.ends_with("test_sale.py"))
        .map(|issue| (issue.at.line.get(), &issue.problem))
        .collect();
    let demo = |xmlid: &str| Problem::DemoRecord {
        xmlid: xmlid.to_string(),
    };
    assert_eq!(
        found,
        [
            (
                10,
                &Problem::RecordGone {
                    xmlid: "sale.action_old".to_string(),
                    how: RefKind::Test,
                }
            ),
            (11, &demo("sale.order_demo")),
            (13, &demo("custom_sale.custom_demo")),
            (15, &demo("sale.order_demo")),
        ]
    );
    let findings: Vec<(PtxCode, Severity, String, usize)> = report(&scan)
        .findings
        .into_iter()
        .filter(|finding| matches!(finding.code, PtxCode::R601 | PtxCode::T802))
        .map(|finding| {
            (
                finding.code,
                finding.severity,
                finding.message,
                finding.locations.len(),
            )
        })
        .collect();
    assert_eq!(
        findings,
        [
            (
                PtxCode::R601,
                Severity::Warning,
                "Record sale.action_old does not exist in 19.0 (env.ref() in tests).".to_string(),
                1
            ),
            (
                PtxCode::T802,
                Severity::Warning,
                "Tests use 2 records (custom_sale.custom_demo, sale.order_demo) that only demo data creates: 19.0 makes databases without demo data unless run with --with-demo.".to_string(),
                3
            ),
        ]
    );
}

#[test]
fn xpaths_and_locators_that_find_nothing_in_the_target_view_are_errors() {
    use pontix_core::PtxCode;

    let xml = |file: &str, body: &str| {
        let parsed = pontix_parsers::parse_xml(file.as_ref(), &format!("<odoo>{body}</odoo>"));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        (PathBuf::from(file), parsed.data)
    };
    let form = |fields: &str| {
        format!(
            "<record id='view_order_form' model='ir.ui.view'><field name='model'>sale.order</field><field name='arch' type='xml'><form><group>{fields}</group></form></field></record>"
        )
    };
    let (mut source, mut target) = (source(), target());
    source.modules[1].data = vec![xml(
        "sale/views.xml",
        &form(
            "<field name='partner_id'/><field name='date_order'/><field name='note'/><button name='action_confirm'/>",
        ),
    )];
    target.modules[1].data = vec![xml("sale/views.xml", &form("<field name='partner_id'/>"))];
    // Views are records of `ir.ui.view`, which base defines.
    for addons in [&mut source, &mut target] {
        addons.modules[0].models.push(model(
            "base/models.py",
            "ir.ui.view",
            Inherit::Absent,
            vec![],
            vec![],
        ));
    }
    let mine = xml(
        "custom_sale/views.xml",
        "<record id='view_order_form' model='ir.ui.view'><field name='inherit_id' ref='sale.view_order_form'/><field name='arch' type='xml'>\
         <xpath expr=\"//field[@name='date_order']\" position='after'><field name='x'/></xpath>\
         <xpath expr=\"//field[@name='partner_id']\" position='after'><field name='y'/></xpath>\
         <xpath expr=\"//field[@name='y']\" position='after'/>\
         <button name='action_confirm' position='attributes'><attribute name='string'>Go</attribute></button>\
         <field name='partner_id' position='after'/>\
         <field name='note' position='after'/>\
         <xpath expr=\"//label[text()='Customer']\" position='after'/>\
         <xpath expr=\"//field[@name='never_there']\" position='after'/>\
         </field></record>",
    );
    for addons in [&mut source, &mut target] {
        let custom = addons
            .modules
            .iter_mut()
            .find(|module| module.name.as_str() == "custom_sale")
            .expect("the custom module");
        custom.data = vec![mine.clone()];
    }
    let scan = scan(&source, &target, &[ModuleName::new("custom_sale")], &[]);
    let gone: Vec<(u32, &Problem)> = scan
        .issues
        .iter()
        .filter(|issue| matches!(issue.problem, Problem::ViewTargetGone { .. }))
        .map(|issue| (issue.at.line.get(), &issue.problem))
        .collect();
    // `y` comes from the view itself, `note` is reported as a
    // field gone from the model, `text()` is not read, and what follows it
    // is not looked at.
    assert_eq!(
        gone,
        [
            (
                1,
                &Problem::ViewTargetGone {
                    view: "sale.view_order_form".to_string(),
                    spec: "//field[@name='date_order']".to_string(),
                    xpath: true,
                }
            ),
            (
                1,
                &Problem::ViewTargetGone {
                    view: "sale.view_order_form".to_string(),
                    spec: "<button name=\"action_confirm\">".to_string(),
                    xpath: false,
                }
            ),
        ]
    );
    assert!(scan.issues.iter().any(|issue| matches!(
        &issue.problem,
        Problem::FieldGone { field, how: PathUse::View(_), .. } if field.as_str() == "note"
    )));
    let finding = report(&scan)
        .findings
        .into_iter()
        .find(|finding| finding.code == PtxCode::X301)
        .expect("an X301 finding");
    assert_eq!(finding.severity, Severity::Error);
    assert_eq!(
        finding.message,
        "XPath //field[@name='date_order'] finds nothing in view sale.view_order_form as 19.0 has it."
    );
}

#[test]
fn a_method_only_another_module_defines_is_not_there_for_this_one() {
    // In 19.0 `sale_hook` defines `_old_hook` again, but only `other`, also
    // being upgraded, installs it: for custom_sale the method is gone.
    let with_hook = |mut addons: Addons, defines: bool| {
        let methods = if defines {
            vec![method("sale_hook/models.py", "_old_hook", &[], 7, 3)]
        } else {
            vec![]
        };
        addons.modules.push(module(
            "sale_hook",
            &["sale"],
            None,
            vec![model(
                "sale_hook/models.py",
                "sale.order",
                extends("sale.order"),
                vec![],
                methods,
            )],
        ));
        addons
            .modules
            .push(module("other", &["sale_hook"], Some("18.0.1.0.0"), vec![]));
        addons
    };
    let upgrading = [ModuleName::new("custom_sale"), ModuleName::new("other")];
    let scan = scan(
        &with_hook(source(), false),
        &with_hook(target(), true),
        &upgrading,
        &[],
    );
    assert!(
        scan.issues
            .iter()
            .any(|issue| issue.module.as_str() == "custom_sale"
                && matches!(
                    &issue.problem,
                    Problem::OverriddenMethodGone { method, .. } if method == "_old_hook"
                )),
        "{:?}",
        scan.issues
    );
}

#[test]
fn field_parameters_the_target_field_class_no_longer_knows_are_warnings() {
    use pontix_core::PtxCode;

    let core = |relational: &str| {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let write = |relative: &str, content: &str| {
            let path = dir.path().join(relative);
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("directories");
            std::fs::write(path, content).expect("a file");
        };
        write("odoo/__init__.py", "");
        write(
            "odoo/fields.py",
            &format!("class Field:\n    string = None\n{relational}"),
        );
        dir
    };
    let (from, to) = (
        core("class Many2one(Field):\n    auto_join = False\n"),
        core("class Many2one(Field):\n    bypass_search_access = False\n"),
    );
    let (mut source, mut target) = (source(), target());
    source.framework = pontix_index::framework(from.path());
    target.framework = pontix_index::framework(to.path());
    let declared = FieldDef {
        params: vec![
            ("auto_join".to_string(), at("custom_sale/models.py", 70)),
            ("string".to_string(), at("custom_sale/models.py", 70)),
            ("tracking".to_string(), at("custom_sale/models.py", 71)),
        ],
        ..field(
            "custom_sale/models.py",
            "reviewer_id",
            FieldType::Many2one,
            Some("res.partner"),
            70,
        )
    };
    for addons in [&mut source, &mut target] {
        let custom = addons
            .modules
            .iter_mut()
            .find(|module| module.name.as_str() == "custom_sale")
            .expect("the custom module");
        custom.models[0].fields.push(declared.clone());
    }
    let scan = scan(&source, &target, &[ModuleName::new("custom_sale")], &[]);
    let gone: Vec<&Problem> = scan
        .issues
        .iter()
        .filter(|issue| matches!(issue.problem, Problem::FieldParameterGone { .. }))
        .map(|issue| &issue.problem)
        .collect();
    // `string` is still known; `tracking` was never a field class's.
    assert_eq!(
        gone,
        [&Problem::FieldParameterGone {
            model: name("sale.order"),
            field: FieldName::new("reviewer_id"),
            field_type: "Many2one".to_string(),
            parameter: "auto_join".to_string(),
        }]
    );
    let finding = report(&scan)
        .findings
        .into_iter()
        .find(|finding| finding.code == PtxCode::F206)
        .expect("an F206 finding");
    assert_eq!(finding.severity, Severity::Warning);
    assert_eq!(finding.locations[0].line.get(), 70);
}

#[test]
fn data_files_that_need_demo_records_are_warnings() {
    use pontix_core::PtxCode;

    const DEMO: &str = "sale/data/sale_demo.xml";
    const DATA: &str = "custom_sale/data/data.xml";
    const MINE: &str = "custom_sale/demo/demo.xml";
    let record = |file: &str, id: &str, line| pontix_core::Record {
        id: Some(id.to_string()),
        model: name("sale.order"),
        fields: Vec::new(),
        view: None,
        location: at(file, line),
    };
    let (mut source, mut target) = (source(), target());
    for addons in [&mut source, &mut target] {
        let sale = &mut addons.modules[1];
        sale.manifest.demo = vec!["data/sale_demo.xml".to_string()];
        sale.data = vec![data(DEMO, vec![record(DEMO, "order_demo", 2)], vec![])];
        let custom = addons
            .modules
            .iter_mut()
            .find(|module| module.name.as_str() == "custom_sale")
            .expect("the custom module");
        custom.manifest.demo = vec!["demo/demo.xml".to_string()];
        custom.data = vec![
            data(
                DATA,
                vec![],
                vec![reference(DATA, "sale.order_demo", RefKind::Ref, 4)],
            ),
            data(
                MINE,
                vec![],
                vec![reference(MINE, "sale.order_demo", RefKind::Ref, 5)],
            ),
        ];
    }
    let scan = scan(&source, &target, &[ModuleName::new("custom_sale")], &[]);
    let found: Vec<(&str, &Problem)> = scan
        .issues
        .iter()
        .filter(|issue| matches!(issue.problem, Problem::DemoRecordInData { .. }))
        .map(|issue| (issue.at.file.to_str().unwrap_or_default(), &issue.problem))
        .collect();
    // The demo file may rely on demo data.
    assert_eq!(
        found,
        [(
            DATA,
            &Problem::DemoRecordInData {
                xmlid: "sale.order_demo".to_string()
            }
        )]
    );
    let finding = report(&scan)
        .findings
        .into_iter()
        .find(|finding| finding.code == PtxCode::R602)
        .expect("an R602 finding");
    assert_eq!(finding.severity, Severity::Warning);
}
