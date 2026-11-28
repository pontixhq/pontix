use std::num::NonZeroU32;
use std::path::PathBuf;

use pontix_core::{
    ApiDecorator, FieldAttrs, FieldDef, FieldPaths, FieldType, Inherit, Manifest, MethodDef,
    ModelBase, ModelDef, OdooVersion, RefKind, Written,
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
        xmlid_refs: Vec::new(),
        data: Vec::new(),
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
