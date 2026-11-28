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
    assert!(text.contains("Not checked: js, xpath"), "{text}");
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

    let unknown = PlanArgs {
        modules: vec!["ghost".to_string()],
        ..trees.args()
    };
    assert!(error(unknown).contains("module ghost is not in the --addons folders"));
}
