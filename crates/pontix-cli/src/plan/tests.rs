use std::fs;

use super::*;

/// Two Odoo source trees and a folder of custom modules, in a temporary
/// directory removed when dropped.
struct Trees {
    dir: tempfile::TempDir,
}

impl Trees {
    fn new() -> Self {
        let trees = Self {
            dir: tempfile::tempdir().expect("a temporary directory"),
        };
        let fields = [
            ("18", "    name = fields.Char()\n    fax = fields.Char()\n"),
            ("19", "    name = fields.Char()\n"),
        ];
        for (version, fields) in fields {
            let core = format!("core{version}");
            trees.write(
                &format!("{core}/odoo/release.py"),
                &format!("version_info = ({version}, 0, 0, FINAL, 0, '')\n"),
            );
            trees.write(
                &format!("{core}/odoo/addons/base/__manifest__.py"),
                "{'name': 'Base'}",
            );
            trees.write(
                &format!("{core}/odoo/addons/base/__init__.py"),
                "from . import models\n",
            );
            trees.write(
                &format!("{core}/odoo/addons/base/models.py"),
                &format!("class Partner(models.Model):\n    _name = 'res.partner'\n{fields}"),
            );
        }
        trees.write(
            "custom/my_partner/__manifest__.py",
            "{'name': 'Mine', 'version': '18.0.1.0.0', 'depends': ['base']}",
        );
        trees.write("custom/my_partner/__init__.py", "from . import models\n");
        for folder in ["18.0.1.0.0", "0.0.0", "1.0.2"] {
            trees.write(
                &format!("custom/my_partner/migrations/{folder}/post-migration.py"),
                "def migrate(cr, version):\n    pass\n",
            );
        }
        trees.write(
            "custom/my_partner/models.py",
            "class Partner(models.Model):\n    _inherit = 'res.partner'\n    fax_copy = fields.Char(related='fax')\n",
        );
        trees
    }

    fn write(&self, relative: &str, content: &str) {
        let path = self.dir.path().join(relative);
        fs::create_dir_all(path.parent().expect("a parent")).expect("parent folders");
        fs::write(path, content).expect("a written file");
    }

    fn args(&self) -> PlanArgs {
        PlanArgs {
            from: OdooVersion::V18,
            to: OdooVersion::V19,
            core_from: self.dir.path().join("core18"),
            core_to: self.dir.path().join("core19"),
            addons: vec![self.dir.path().join("custom")],
            deps: Vec::new(),
            deps_to: Vec::new(),
            modules: Vec::new(),
            format: Format::Text,
            output: None,
            fail_on: None,
        }
    }
}

#[test]
fn reports_findings_as_text_with_relative_paths() {
    let trees = Trees::new();
    let (text, outcome) = plan(&trees.args(), false).expect("a report");
    assert!(!outcome.failed);
    assert!(
        text.contains("Pontix plan 18.0 -> 19.0 · 1 module(s) checked"),
        "{text}"
    );
    assert!(
        text.contains("PTX-F201  Field res.partner.fax does not exist in 19.0"),
        "{text}"
    );
    assert!(text.contains("at: my_partner/models.py:3"), "{text}");
    assert!(
        text.contains("evidence: [18.0] odoo/addons/base/models.py:4 · gone in 19.0"),
        "{text}"
    );
    assert!(text.contains("PTX-M002"), "{text}");
    // 0.0.0 runs on every update, 1.0.2 is read as 19.0.1.0.2: only the
    // 18.0 folder is stale.
    assert!(
        text.contains("PTX-M003  migrations/18.0.1.0.0 is for a version below 19.0"),
        "{text}"
    );
    assert_eq!(text.matches("PTX-M003").count(), 1, "{text}");
    assert!(text.contains("Not checked: js, view expressions"), "{text}");
    let root = trees.dir.path().to_string_lossy().into_owned();
    assert!(!text.contains(&root), "absolute paths leak into the report");
}

#[test]
fn reports_findings_as_json_with_a_schema_version() {
    let trees = Trees::new();
    let args = PlanArgs {
        format: Format::Json,
        ..trees.args()
    };
    let (json, _) = plan(&args, false).expect("a report");
    let document: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
    assert_eq!(document["schema_version"], 1);
    assert_eq!(document["from"], "18.0");
    let field = document["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .find(|finding| finding["code"] == "PTX-F201")
        .expect("the missing field");
    assert_eq!(field["scope"], "custom");
    assert_eq!(field["locations"][0]["root"], "addons");
    assert_eq!(field["locations"][0]["path"], "my_partner/models.py");
    assert_eq!(field["basis"]["kind"], "core");
    assert_eq!(field["basis"]["before"]["root"], "core_from");
    assert!(field["basis"]["after"].is_null());
    let root = trees.dir.path().to_string_lossy().into_owned();
    assert!(!json.contains(&root), "absolute paths leak into the report");
}

#[test]
fn modules_of_the_same_folders_upgrade_together() {
    let trees = Trees::new();
    trees.write(
        "custom/my_base/__manifest__.py",
        "{'name': 'Base mixin', 'version': '18.0.1.0.0', 'depends': ['base']}",
    );
    trees.write("custom/my_base/__init__.py", "");
    trees.write(
        "custom/my_partner/__manifest__.py",
        "{'name': 'Mine', 'version': '18.0.1.0.0', 'depends': ['base', 'my_base']}",
    );
    let args = PlanArgs {
        modules: vec!["my_partner".to_string()],
        ..trees.args()
    };
    let (text, _) = plan(&args, false).expect("a report");
    assert!(!text.contains("PTX-M001"), "{text}");
    assert!(!text.contains("\nmy_base\n"), "{text}");
    assert!(text.contains("1 module(s) checked"), "{text}");
}

#[test]
fn fail_on_decides_the_exit() {
    let trees = Trees::new();
    let args = PlanArgs {
        fail_on: Some(FailOn::Error),
        ..trees.args()
    };
    let (_, outcome) = plan(&args, false).expect("a report");
    assert!(outcome.failed);
}

#[test]
fn refuses_input_it_cannot_work_with() {
    let trees = Trees::new();
    let error = |args: PlanArgs| plan(&args, false).expect_err("refused");

    let swapped = PlanArgs {
        core_from: trees.dir.path().join("core19"),
        ..trees.args()
    };
    assert!(error(swapped).contains("holds Odoo 19.0, not 18.0"));

    let backwards = PlanArgs {
        from: OdooVersion::V19,
        to: OdooVersion::V18,
        ..trees.args()
    };
    assert!(error(backwards).contains("one version at a time"));

    let not_odoo = PlanArgs {
        core_to: trees.dir.path().join("custom"),
        ..trees.args()
    };
    assert!(error(not_odoo).contains("is not an Odoo source tree"));

    let deps_to_alone = PlanArgs {
        deps_to: vec![trees.dir.path().join("custom")],
        ..trees.args()
    };
    assert!(error(deps_to_alone).contains("--deps-to needs --deps"));

    let unknown = PlanArgs {
        modules: vec!["ghost".to_string()],
        ..trees.args()
    };
    assert!(error(unknown).contains("module ghost is not in the --addons folders"));
}

#[test]
fn third_party_dependencies_are_checked_in_their_target_version() {
    let trees = Trees::new();
    for (version, field) in [("18", "    oca_code = fields.Char()\n"), ("19", "")] {
        let dep = format!("oca{version}/oca_partner");
        trees.write(
            &format!("{dep}/__manifest__.py"),
            &format!("{{'name': 'OCA', 'version': '{version}.0.1.0.0', 'depends': ['base']}}"),
        );
        trees.write(&format!("{dep}/__init__.py"), "from . import models\n");
        trees.write(
            &format!("{dep}/models.py"),
            &format!("class Partner(models.Model):\n    _inherit = 'res.partner'\n{field}"),
        );
    }
    // Renamed in the target version: only the source has it.
    trees.write(
        "oca18/oca_old/__manifest__.py",
        "{'name': 'Old', 'version': '18.0.1.0.0', 'depends': ['base']}",
    );
    trees.write("oca18/oca_old/__init__.py", "");
    trees.write(
        "custom/my_partner/__manifest__.py",
        "{'name': 'Mine', 'version': '18.0.1.0.0', 'depends': ['oca_partner', 'oca_old']}",
    );
    trees.write(
        "custom/my_partner/models.py",
        "class Partner(models.Model):\n    _inherit = 'res.partner'\n    code_copy = fields.Char(related='oca_code')\n",
    );
    let args = PlanArgs {
        deps: vec![trees.dir.path().join("oca18")],
        deps_to: vec![trees.dir.path().join("oca19")],
        format: Format::Json,
        ..trees.args()
    };
    let (json, _) = plan(&args, false).expect("a report");
    let document: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
    let findings = document["findings"].as_array().expect("findings");
    let find = |code: &str, subject: &str| {
        findings
            .iter()
            .find(|finding| finding["code"] == code && finding["subject"] == subject)
            .unwrap_or_else(|| panic!("{code} {subject} in {json}"))
    };
    let field = find("PTX-F201", "res.partner.oca_code");
    assert_eq!(field["scope"], "custom");
    assert_eq!(field["basis"]["before"]["root"], "deps");
    assert_eq!(field["basis"]["before"]["path"], "oca_partner/models.py");
    assert_eq!(find("PTX-M001", "oca_old")["module"], "my_partner");
    // The dependencies are not upgraded: nothing is said about their own
    // manifests.
    assert!(
        !findings
            .iter()
            .any(|finding| finding["module"] == "oca_partner" && finding["code"] == "PTX-M002"),
        "{json}"
    );
    assert!(
        document["modules"]
            .as_array()
            .is_none_or(|modules| modules.len() == 1)
    );

    // Without --deps-to the dependencies are taken as unchanged.
    let unchanged = PlanArgs {
        deps_to: Vec::new(),
        ..args
    };
    let (json, _) = plan(&unchanged, false).expect("a report");
    assert!(!json.contains("res.partner.oca_code"), "{json}");
    assert!(!json.contains("\"oca_old\""), "{json}");
}
