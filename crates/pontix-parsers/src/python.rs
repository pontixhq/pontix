//! Python source files: Odoo model classes and what they declare.
//!
//! Only module-level classes are read, because that is where Odoo defines
//! models. Whatever cannot be read with certainty becomes a [`Diagnostic`]
//! carrying the source text exactly as written.

mod field;
mod method;
mod module_level;

use std::path::Path;

use pontix_core::{
    Delegation, Diagnostic, DiagnosticKind, FieldDef, FieldName, Inherit, MethodDef, ModelDef,
    ModelName,
};
use ruff_python_ast::visitor::{Visitor, walk_stmt};
use ruff_python_ast::{Expr, ExprList, ExprName, ExprTuple, Stmt, StmtClassDef};
use ruff_python_parser::parse_module;
use ruff_text_size::{Ranged, TextRange, TextSize};

use self::field::{Comodel, Field};
pub use self::module_level::Import;
use crate::source::SourceFile;

/// Classes from `odoo.models` that make a subclass an Odoo model. Core uses
/// `BaseModel` directly too, for example in `hr_skills` reports (Odoo 19).
const MODEL_BASES: [&str; 4] = ["Model", "AbstractModel", "TransientModel", "BaseModel"];

/// Class attributes that define a model's identity and inheritance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Attribute {
    Name,
    Inherit,
    Inherits,
}

const MODEL_ATTRIBUTES: [(&str, Attribute); 3] = [
    ("_name", Attribute::Name),
    ("_inherit", Attribute::Inherit),
    ("_inherits", Attribute::Inherits),
];

/// A module-level class whose bases are not model classes this file can see,
/// for example `class CheckIdentityPasskeys(CheckIdentity)` with
/// `CheckIdentity` imported from `base` (`auth_passkey`, Odoo 18). Python
/// makes it a model when one of its bases is a model class; the indexer
/// decides once every file is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The class, read as if it were a model.
    pub model: ModelDef,
    /// Its bases written as names or dotted names, such as `CheckIdentity`
    /// or `res_users.CheckIdentity`.
    pub bases: Vec<String>,
    /// Whether the body sets `_name`, `_inherit` or `_inherits`.
    pub sets_model_attributes: bool,
    /// What could not be read in the body; it counts only for a model.
    pub diagnostics: Vec<Diagnostic>,
    /// The report for a class that sets model attributes but turns out to
    /// have no model base.
    pub unrecognized: Diagnostic,
}

/// What one Python file declares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PythonFile {
    /// Model classes, in source order.
    pub models: Vec<ModelDef>,
    /// Classes that may be models through a base defined in another file.
    pub candidates: Vec<Candidate>,
    /// Imports that run when the file is imported, in source order.
    pub imports: Vec<Import>,
    /// Names bound at module level when the file is imported: assignments,
    /// functions, classes and import aliases. `from . import x` may import
    /// such a name from a package's `__init__.py` instead of a file.
    pub bound_names: Vec<String>,
    /// Everything that could not be read, in source order.
    pub diagnostics: Vec<Diagnostic>,
}

/// Reads the Odoo models declared in one Python source file.
///
/// `file` is only recorded in locations; the file is never opened here.
/// A file that is not valid Python yields a single
/// [`DiagnosticKind::SyntaxError`] and no models or imports, because Odoo
/// cannot import it.
pub fn parse_python(file: &Path, source: &str) -> PythonFile {
    let mut reader = Reader {
        source: SourceFile::new(file, source),
        out: PythonFile::default(),
    };
    match parse_module(source) {
        Ok(parsed) => {
            let module_level = module_level::module_level(&reader.source, &parsed.syntax().body);
            reader.out.imports = module_level.imports;
            reader.out.bound_names = module_level.bound_names;
            for stmt in &parsed.syntax().body {
                match stmt {
                    Stmt::ClassDef(class) => reader.class(class),
                    Stmt::FunctionDef(_) => {}
                    compound => reader.nested_classes(compound),
                }
            }
        }
        Err(error) => reader.report_line(DiagnosticKind::SyntaxError, error.location.start()),
    }
    reader.out
}

/// One file being read; collects models and diagnostics as it goes.
struct Reader<'a> {
    source: SourceFile<'a>,
    out: PythonFile,
}

impl Reader<'_> {
    fn class(&mut self, class: &StmtClassDef) {
        let bases: Vec<&Expr> = class
            .arguments
            .as_deref()
            .map(|arguments| arguments.args.iter().collect())
            .unwrap_or_default();
        if bases
            .iter()
            .any(|base| is_model_base(base) || self.is_model_class_in_this_file(base))
        {
            let model = self.read_model(class);
            self.out.models.push(model);
            return;
        }

        let sets_model_attributes = class
            .body
            .iter()
            .any(|stmt| model_attribute(stmt).is_some());
        let written: Vec<String> = bases.iter().filter_map(|base| dotted_name(base)).collect();
        if written.is_empty() {
            // A plain Python class is none of our business, unless it sets the
            // attributes only a model uses: then its base is one we cannot see.
            if sets_model_attributes {
                self.report_line(
                    DiagnosticKind::UnrecognizedModelClass,
                    class.name.range.start(),
                );
            }
            return;
        }

        // Read it as a model, keeping its diagnostics apart: they only count
        // if the indexer finds that a base is a model class.
        let before = self.out.diagnostics.len();
        let model = self.read_model(class);
        let diagnostics = self.out.diagnostics.split_off(before);
        self.out.candidates.push(Candidate {
            model,
            bases: written,
            sets_model_attributes,
            diagnostics,
            unrecognized: self.source.line_diagnostic(
                DiagnosticKind::UnrecognizedModelClass,
                class.name.range.start(),
            ),
        });
    }

    /// Reads a class body as a model: identity, inheritance, fields, methods.
    fn read_model(&mut self, class: &StmtClassDef) -> ModelDef {
        let mut model = ModelDef {
            class_name: class.name.as_str().to_string(),
            name: None,
            inherit: Inherit::Absent,
            inherits: Vec::new(),
            fields: Vec::new(),
            methods: Vec::new(),
            location: self.source.location(class.name.range.start()),
        };

        // Statements run top to bottom, so a later assignment replaces an
        // earlier one, and one we cannot read leaves the value unknown.
        for stmt in &class.body {
            if let Stmt::FunctionDef(def) = stmt {
                model.methods.push(MethodDef {
                    name: def.name.as_str().to_string(),
                    params: method::params(def),
                    calls_super: method::calls_super(def),
                    location: self.source.location(def.name.range.start()),
                });
                continue;
            }
            if let Some((target, value)) = field_assignment(stmt) {
                self.field(&mut model, stmt, target, value);
                continue;
            }
            let Some(assignment) = model_attribute(stmt) else {
                continue;
            };
            match assignment.attribute {
                Attribute::Name => {
                    model.name = assignment.value.and_then(string).map(ModelName::new);
                    if model.name.is_none() {
                        self.report(DiagnosticKind::UnrecognizedName, assignment.range);
                    }
                }
                Attribute::Inherit => match assignment
                    .value
                    .and_then(|value| inherit(value, model.name.as_ref()))
                {
                    Some(inherit) => model.inherit = inherit,
                    None => {
                        model.inherit = Inherit::Absent;
                        self.report(DiagnosticKind::UnrecognizedInherit, assignment.range);
                    }
                },
                Attribute::Inherits => match assignment.value.and_then(delegations) {
                    Some(inherits) => model.inherits = inherits,
                    None => {
                        model.inherits.clear();
                        self.report(DiagnosticKind::UnrecognizedInherits, assignment.range);
                    }
                },
            }
        }

        model
    }

    /// Reports model classes defined inside a module-level compound
    /// statement, such as `for name in MODELS: class NewModel(models.Model)`
    /// in `test_import_export` (Odoo 19).
    fn nested_classes(&mut self, stmt: &Stmt) {
        let mut finder = NestedClasses {
            reader: self,
            found: Vec::new(),
        };
        finder.visit_stmt(stmt);
        for offset in finder.found {
            self.report_line(DiagnosticKind::NestedModelClass, offset);
        }
    }

    /// Adds `target = value` to the model when the value is a field.
    fn field(&mut self, model: &mut ModelDef, stmt: &Stmt, target: &ExprName, value: &Expr) {
        let Some(declaration) = field::field(value, model.name.as_ref()) else {
            return;
        };
        let Field::Declared {
            field_type,
            comodel,
        } = declaration
        else {
            self.report(DiagnosticKind::UnrecognizedField, stmt.range());
            return;
        };
        let comodel = match comodel {
            Comodel::Absent => None,
            Comodel::Named(name) => Some(name),
            Comodel::Unreadable => {
                // The field exists either way; only its comodel is unknown.
                self.report(DiagnosticKind::UnrecognizedComodel, stmt.range());
                None
            }
        };
        model.fields.push(FieldDef {
            name: FieldName::new(target.id.as_str()),
            field_type,
            comodel,
            location: self.source.location(target.range.start()),
        });
    }

    /// A base written as the name of a model class defined earlier in this
    /// file, for example `class WebsitePublishedMultiMixin(WebsitePublishedMixin)`
    /// in `website/models/mixins.py`. Python only sees classes defined above,
    /// so earlier models are the only ones that count. Bases imported from
    /// other files stay unrecognised here.
    fn is_model_class_in_this_file(&self, base: &Expr) -> bool {
        let Expr::Name(name) = base else {
            return false;
        };
        self.out
            .models
            .iter()
            .any(|model| model.class_name == name.id.as_str())
    }

    fn report(&mut self, kind: DiagnosticKind, range: TextRange) {
        let diagnostic = self.source.diagnostic(kind, range);
        self.out.diagnostics.push(diagnostic);
    }

    fn report_line(&mut self, kind: DiagnosticKind, offset: TextSize) {
        let diagnostic = self.source.line_diagnostic(kind, offset);
        self.out.diagnostics.push(diagnostic);
    }
}

/// `name = value` or `name: T = value` with a single, non-underscore name,
/// the only shapes a field declaration takes. Names starting with `_` are
/// model attributes (`_name`, `_order`, ...), never fields.
fn field_assignment(stmt: &Stmt) -> Option<(&ExprName, &Expr)> {
    let (target, value) = match stmt {
        Stmt::Assign(assign) => match assign.targets.as_slice() {
            [target] => (target, &*assign.value),
            _ => return None,
        },
        Stmt::AnnAssign(assign) => (&*assign.target, assign.value.as_deref()?),
        _ => return None,
    };
    match target {
        Expr::Name(name) if !name.id.as_str().starts_with('_') => Some((name, value)),
        _ => None,
    }
}

/// Finds model classes inside a compound statement; functions are skipped
/// because their bodies only run when called.
struct NestedClasses<'r, 'a> {
    reader: &'r Reader<'a>,
    found: Vec<TextSize>,
}

impl<'s> Visitor<'s> for NestedClasses<'_, '_> {
    fn visit_stmt(&mut self, stmt: &'s Stmt) {
        match stmt {
            Stmt::ClassDef(class) => {
                let is_model = class.arguments.as_deref().is_some_and(|arguments| {
                    arguments.args.iter().any(|base| {
                        is_model_base(base) || self.reader.is_model_class_in_this_file(base)
                    })
                });
                if is_model {
                    self.found.push(class.name.range.start());
                }
            }
            Stmt::FunctionDef(_) => {}
            other => walk_stmt(self, other),
        }
    }
}

/// An assignment to `_name`, `_inherit` or `_inherits` in a class body.
struct ModelAttribute<'a> {
    attribute: Attribute,
    /// `None` when the statement changes the value in a way that cannot be
    /// read statically, for example `_inherit += [...]`.
    value: Option<&'a Expr>,
    range: TextRange,
}

/// Recognises `x = ...`, `x: T = ...` and `x += ...` for the model attributes.
/// An annotation without a value assigns nothing and is skipped.
fn model_attribute(stmt: &Stmt) -> Option<ModelAttribute<'_>> {
    let (attribute, value) = match stmt {
        Stmt::Assign(assign) => (
            assign.targets.iter().find_map(attribute_name)?,
            Some(&*assign.value),
        ),
        Stmt::AnnAssign(assign) => (
            attribute_name(&assign.target)?,
            Some(assign.value.as_deref()?),
        ),
        Stmt::AugAssign(assign) => (attribute_name(&assign.target)?, None),
        _ => return None,
    };
    Some(ModelAttribute {
        attribute,
        value,
        range: stmt.range(),
    })
}

fn attribute_name(target: &Expr) -> Option<Attribute> {
    let Expr::Name(name) = target else {
        return None;
    };
    MODEL_ATTRIBUTES
        .into_iter()
        .find_map(|(written, attribute)| (name.id.as_str() == written).then_some(attribute))
}

/// `Name` or `a.b.Name`, as written; `None` for anything else, such as a
/// call or a subscript.
fn dotted_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Name(name) => Some(name.id.as_str().to_string()),
        Expr::Attribute(attribute) => Some(format!(
            "{}.{}",
            dotted_name(&attribute.value)?,
            attribute.attr.as_str()
        )),
        _ => None,
    }
}

/// `models.Model`, `odoo.models.Model` or a bare `Model`, and the same for
/// every other name in [`MODEL_BASES`].
fn is_model_base(base: &Expr) -> bool {
    match base {
        Expr::Name(name) => MODEL_BASES.contains(&name.id.as_str()),
        Expr::Attribute(attribute) => {
            let from_models = match &*attribute.value {
                Expr::Name(module) => module.id.as_str() == "models",
                Expr::Attribute(module) => module.attr.as_str() == "models",
                _ => false,
            };
            from_models && MODEL_BASES.contains(&attribute.attr.as_str())
        }
        _ => false,
    }
}

/// A string literal, implicit concatenation included; nothing else.
fn string(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::StringLiteral(literal) => Some(literal.value.to_str()),
        _ => None,
    }
}

/// A string, or a list or tuple whose items are all strings. `_name` refers
/// to the value assigned to `_name` above it in the same class body, as in
/// `_inherit = _name` (`website/models/ir_module_module.py`, Odoo 18).
fn inherit(expr: &Expr, name: Option<&ModelName>) -> Option<Inherit> {
    match expr {
        Expr::Name(reference) if reference.id.as_str() == "_name" => {
            name.cloned().map(Inherit::Single)
        }
        Expr::StringLiteral(literal) => {
            Some(Inherit::Single(ModelName::new(literal.value.to_str())))
        }
        Expr::List(ExprList { elts, .. }) | Expr::Tuple(ExprTuple { elts, .. }) => elts
            .iter()
            .map(|item| string(item).map(ModelName::new))
            .collect::<Option<Vec<_>>>()
            .map(Inherit::List),
        _ => None,
    }
}

/// A dict literal whose keys and values are all strings. A repeated key keeps
/// its first position and its last value, exactly as a Python dict does.
fn delegations(expr: &Expr) -> Option<Vec<Delegation>> {
    let Expr::Dict(dict) = expr else {
        return None;
    };
    let mut entries: Vec<Delegation> = Vec::with_capacity(dict.items.len());
    for item in &dict.items {
        let parent = ModelName::new(string(item.key.as_ref()?)?);
        let field = FieldName::new(string(&item.value)?);
        match entries.iter_mut().find(|entry| entry.parent == parent) {
            Some(entry) => entry.field = field,
            None => entries.push(Delegation { parent, field }),
        }
    }
    Some(entries)
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use pontix_core::Location;

    use super::*;

    const FILE: &str = "models/sale_order.py";

    fn parse(lines: &[&str]) -> PythonFile {
        parse_python(Path::new(FILE), &lines.join("\n"))
    }

    fn only_model(file: &PythonFile) -> &ModelDef {
        assert_eq!(file.models.len(), 1, "expected one model: {file:?}");
        &file.models[0]
    }

    fn name(value: &str) -> ModelName {
        ModelName::new(value)
    }

    #[test]
    fn reads_name_inherit_and_inherits() {
        let file = parse(&[
            "from odoo import fields, models",
            "",
            "",
            "class ResUsers(models.Model):",
            "    _name = 'res.users'",
            "    _inherit = ['mail.thread', 'avatar.mixin']",
            "    _inherits = {'res.partner': 'partner_id'}",
        ]);
        let model = only_model(&file);
        assert_eq!(model.class_name, "ResUsers");
        assert_eq!(model.name, Some(name("res.users")));
        assert_eq!(
            model.inherit,
            Inherit::List(vec![name("mail.thread"), name("avatar.mixin")])
        );
        assert_eq!(
            model.inherits,
            [Delegation {
                parent: name("res.partner"),
                field: FieldName::new("partner_id"),
            }]
        );
        assert_eq!(model.location.to_string(), format!("{FILE}:4"));
        assert!(file.diagnostics.is_empty());
    }

    #[test]
    fn string_inherit_is_single_and_tuple_is_a_list() {
        let file = parse(&[
            "class A(models.Model):",
            "    _inherit = 'sale.order'",
            "class B(models.Model):",
            "    _inherit = ('sale.order',)",
        ]);
        assert_eq!(file.models[0].inherit, Inherit::Single(name("sale.order")));
        assert_eq!(
            file.models[1].inherit,
            Inherit::List(vec![name("sale.order")])
        );
    }

    #[test]
    fn recognises_every_model_base() {
        let file = parse(&[
            "class A(models.Model): pass",
            "class B(models.AbstractModel): pass",
            "class C(models.TransientModel): pass",
            "class D(odoo.models.Model): pass",
            "class E(Model): pass",
            "class F(models.Model, SomeMixin): pass",
            "class G(models.BaseModel): pass",
        ]);
        let classes: Vec<&str> = file.models.iter().map(|m| m.class_name.as_str()).collect();
        assert_eq!(classes, ["A", "B", "C", "D", "E", "F", "G"]);
    }

    #[test]
    fn a_model_class_defined_earlier_counts_as_a_base() {
        let file = parse(&[
            "class PublishedMixin(models.AbstractModel):",
            "    _name = 'published.mixin'",
            "class PublishedMultiMixin(PublishedMixin):",
            "    _name = 'published.multi.mixin'",
        ]);
        let names: Vec<Option<&str>> = file
            .models
            .iter()
            .map(|model| model.name.as_ref().map(ModelName::as_str))
            .collect();
        assert_eq!(
            names,
            [Some("published.mixin"), Some("published.multi.mixin")]
        );
        assert!(file.diagnostics.is_empty());
    }

    #[test]
    fn a_model_class_defined_later_does_not_count() {
        let file = parse(&[
            "class Early(Late):",
            "    _name = 'early'",
            "class Late(models.Model):",
            "    _name = 'late'",
        ]);
        assert_eq!(only_model(&file).class_name, "Late");
        assert_eq!(file.candidates[0].bases, ["Late"]);
    }

    #[test]
    fn annotated_assignment_is_read_and_bare_annotation_is_not() {
        let file = parse(&[
            "class A(models.Model):",
            "    _name: str = 'a.model'",
            "    _inherit: list",
        ]);
        let model = only_model(&file);
        assert_eq!(model.name, Some(name("a.model")));
        assert_eq!(model.inherit, Inherit::Absent);
        assert!(file.diagnostics.is_empty());
    }

    #[test]
    fn inherit_may_refer_to_name_assigned_above() {
        let file = parse(&[
            "class A(models.Model):",
            "    _name = 'ir.module.module'",
            "    _inherit = _name",
            "class B(models.Model):",
            "    _inherit = _name",
        ]);
        assert_eq!(
            file.models[0].inherit,
            Inherit::Single(name("ir.module.module"))
        );
        assert_eq!(file.models[1].inherit, Inherit::Absent);
        assert_eq!(file.diagnostics.len(), 1);
        assert_eq!(file.diagnostics[0].snippet, "_inherit = _name");
    }

    #[test]
    fn last_assignment_wins() {
        let file = parse(&[
            "class A(models.Model):",
            "    _name = 'first'",
            "    _name = 'second'",
        ]);
        assert_eq!(only_model(&file).name, Some(name("second")));
    }

    #[test]
    fn repeated_inherits_key_keeps_its_last_value() {
        let file = parse(&[
            "class A(models.Model):",
            "    _inherits = {'res.partner': 'old_id', 'res.company': 'company_id', 'res.partner': 'partner_id'}",
        ]);
        let fields: Vec<&str> = only_model(&file)
            .inherits
            .iter()
            .map(|entry| entry.field.as_str())
            .collect();
        assert_eq!(fields, ["partner_id", "company_id"]);
    }

    #[test]
    fn unreadable_values_are_reported_verbatim() {
        let file = parse(&[
            "class A(models.Model):",
            "    _name = MODEL_NAME",
            "    _inherit = [",
            "        'mail.thread',",
            "        BASE,",
            "    ]",
            "    _inherits = {**PARENTS}",
            "    _inherit += ['x']",
        ]);
        let model = only_model(&file);
        assert_eq!(model.name, None);
        assert_eq!(model.inherit, Inherit::Absent);
        assert!(model.inherits.is_empty());

        let seen: Vec<(DiagnosticKind, String, &str)> = file
            .diagnostics
            .iter()
            .map(|d| (d.kind, d.location.to_string(), d.snippet.as_str()))
            .collect();
        assert_eq!(
            seen,
            [
                (
                    DiagnosticKind::UnrecognizedName,
                    format!("{FILE}:2"),
                    "_name = MODEL_NAME"
                ),
                (
                    DiagnosticKind::UnrecognizedInherit,
                    format!("{FILE}:3"),
                    "_inherit = [\n        'mail.thread',\n        BASE,\n    ]",
                ),
                (
                    DiagnosticKind::UnrecognizedInherits,
                    format!("{FILE}:7"),
                    "_inherits = {**PARENTS}"
                ),
                (
                    DiagnosticKind::UnrecognizedInherit,
                    format!("{FILE}:8"),
                    "_inherit += ['x']"
                ),
            ]
        );
    }

    #[test]
    fn a_later_unreadable_assignment_clears_the_earlier_value() {
        let file = parse(&[
            "class A(models.Model):",
            "    _inherit = 'sale.order'",
            "    _inherit = PARENT",
        ]);
        assert_eq!(only_model(&file).inherit, Inherit::Absent);
        assert_eq!(file.diagnostics.len(), 1);
    }

    #[test]
    fn plain_classes_are_ignored() {
        let file = parse(&[
            "class Helper:",
            "    _cache = {}",
            "class Wizard(models.Model):",
            "    class Inner(models.Model):",
            "        _name = 'not.module.level'",
        ]);
        assert_eq!(only_model(&file).class_name, "Wizard");
        assert!(file.diagnostics.is_empty());
    }

    #[test]
    fn classes_on_unknown_bases_become_candidates() {
        let file = parse(&[
            "class SaleOrder(CustomBase):",
            "    _inherit = 'sale.order'",
            "    note = Char()",
            "class Passkeys(res_users.CheckIdentity):",
            "    pass",
            "class Built(make_base()):",
            "    _inherit = 'res.partner'",
        ]);
        assert!(file.models.is_empty());
        let candidates: Vec<(&str, Vec<&str>, bool, usize)> = file
            .candidates
            .iter()
            .map(|c| {
                (
                    c.model.class_name.as_str(),
                    c.bases.iter().map(String::as_str).collect(),
                    c.sets_model_attributes,
                    c.diagnostics.len(),
                )
            })
            .collect();
        assert_eq!(
            candidates,
            [
                ("SaleOrder", vec!["CustomBase"], true, 1),
                ("Passkeys", vec!["res_users.CheckIdentity"], false, 0),
            ]
        );
        assert_eq!(
            file.candidates[0].model.inherit,
            Inherit::Single(name("sale.order"))
        );
        assert_eq!(
            file.candidates[0].unrecognized.snippet,
            "class SaleOrder(CustomBase):"
        );
        // A base that is not a name cannot be resolved later either.
        let seen: Vec<(DiagnosticKind, &str)> = file
            .diagnostics
            .iter()
            .map(|d| (d.kind, d.snippet.as_str()))
            .collect();
        assert_eq!(
            seen,
            [(
                DiagnosticKind::UnrecognizedModelClass,
                "class Built(make_base()):"
            )]
        );
    }

    #[test]
    fn invalid_python_is_one_syntax_error_and_no_models() {
        let file = parse(&[
            "class A(models.Model):",
            "    _name = ",
            "class B(models.Model): pass",
        ]);
        assert!(file.models.is_empty());
        assert_eq!(file.diagnostics.len(), 1);
        assert_eq!(file.diagnostics[0].kind, DiagnosticKind::SyntaxError);
        assert_eq!(
            file.diagnostics[0].location.to_string(),
            format!("{FILE}:2")
        );
        assert_eq!(file.diagnostics[0].snippet, "    _name = ");
    }

    #[test]
    fn reads_fields_and_methods_in_source_order() {
        let file = parse(&[
            "class SaleOrder(models.Model):",
            "    _inherit = 'sale.order'",
            "",
            "    partner_id = fields.Many2one(ondelete='restrict')",
            "    carrier_id: fields.Many2one = fields.Many2one('delivery.carrier')",
            "    _sql_constraints = [('name_uniq', 'unique(name)', 'Must be unique')]",
            "    _name_uniq = models.Constraint('UNIQUE(name)', 'Must be unique')",
            "",
            "    def action_confirm(self):",
            "        return super().action_confirm()",
        ]);
        let model = only_model(&file);
        let fields: Vec<(&str, &str, Option<&str>, String)> = model
            .fields
            .iter()
            .map(|f| {
                (
                    f.name.as_str(),
                    f.field_type.as_str(),
                    f.comodel.as_ref().map(ModelName::as_str),
                    f.location.to_string(),
                )
            })
            .collect();
        assert_eq!(
            fields,
            [
                ("partner_id", "Many2one", None, format!("{FILE}:4")),
                (
                    "carrier_id",
                    "Many2one",
                    Some("delivery.carrier"),
                    format!("{FILE}:5")
                ),
            ]
        );
        assert_eq!(
            model.methods,
            [MethodDef {
                name: "action_confirm".to_string(),
                params: vec![],
                calls_super: true,
                location: Location::new(FILE, NonZeroU32::new(9).expect("nine is not zero")),
            }]
        );
        assert!(file.diagnostics.is_empty());
    }

    #[test]
    fn unreadable_fields_are_reported_and_unreadable_comodels_keep_the_field() {
        let file = parse(&[
            "class A(models.Model):",
            "    name = Char(string='Name')",
            "    partner_id = fields.Many2one(PARTNER)",
        ]);
        let model = only_model(&file);
        assert_eq!(model.fields.len(), 1);
        assert_eq!(model.fields[0].name.as_str(), "partner_id");
        assert_eq!(model.fields[0].comodel, None);
        let kinds: Vec<(DiagnosticKind, &str)> = file
            .diagnostics
            .iter()
            .map(|d| (d.kind, d.snippet.as_str()))
            .collect();
        assert_eq!(
            kinds,
            [
                (
                    DiagnosticKind::UnrecognizedField,
                    "name = Char(string='Name')"
                ),
                (
                    DiagnosticKind::UnrecognizedComodel,
                    "partner_id = fields.Many2one(PARTNER)"
                ),
            ]
        );
    }

    #[test]
    fn collects_imports_that_run_on_import() {
        let file = parse(&[
            "from . import models, wizard",
            "from .models import sale_order as so",
            "from .. import tools",
            "import odoo.addons.sale.models, logging",
            "try:",
            "    from . import optional",
            "except ImportError:",
            "    from .vendor import fallback",
            "def helper():",
            "    from . import lazy",
            "class Thing:",
            "    from . import inside_class",
        ]);
        let seen: Vec<(u32, Option<&str>, Vec<&str>, String)> = file
            .imports
            .iter()
            .map(|import| {
                (
                    import.level,
                    import.module.as_deref(),
                    import.names.iter().map(String::as_str).collect(),
                    import.location.to_string(),
                )
            })
            .collect();
        assert_eq!(
            seen,
            [
                (1, None, vec!["models", "wizard"], format!("{FILE}:1")),
                (1, Some("models"), vec!["sale_order"], format!("{FILE}:2")),
                (2, None, vec!["tools"], format!("{FILE}:3")),
                (
                    0,
                    Some("odoo.addons.sale.models"),
                    vec![],
                    format!("{FILE}:4")
                ),
                (0, Some("logging"), vec![], format!("{FILE}:4")),
                (1, None, vec!["optional"], format!("{FILE}:6")),
            ]
        );
        assert_eq!(
            file.imports[1].statement,
            "from .models import sale_order as so"
        );
        assert_eq!(
            file.bound_names,
            [
                "models", "wizard", "so", "tools", "odoo", "logging", "optional", "helper", "Thing"
            ]
        );
        assert_eq!(file.imports[1].bindings, ["so"]);
        assert_eq!(file.imports[3].bindings, ["odoo"]);
    }

    #[test]
    fn bound_names_cover_assignments_and_unpacking() {
        let file = parse(&[
            "NOTICE = 'deprecated'",
            "a, (b, *rest) = 1, (2, 3)",
            "TIMEOUT: int = 5",
            "declared_only: int",
            "if True:",
            "    FLAG = True",
        ]);
        assert_eq!(
            file.bound_names,
            ["NOTICE", "a", "b", "rest", "TIMEOUT", "FLAG"]
        );
    }

    #[test]
    fn model_classes_in_compound_statements_are_reported_not_read() {
        let file = parse(&[
            "for name, field in MODELS:",
            "    class NewModel(models.Model):",
            "        _name = f'export.{name}'",
            "if DEBUG:",
            "    class Helper:",
            "        pass",
            "def factory():",
            "    class Built(models.Model):",
            "        pass",
        ]);
        assert!(file.models.is_empty());
        let seen: Vec<(DiagnosticKind, &str)> = file
            .diagnostics
            .iter()
            .map(|d| (d.kind, d.snippet.as_str()))
            .collect();
        assert_eq!(
            seen,
            [(
                DiagnosticKind::NestedModelClass,
                "    class NewModel(models.Model):"
            )]
        );
    }
}
