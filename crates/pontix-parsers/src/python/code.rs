//! The fields and models a function body uses, wherever the model is certain.
//!
//! A small, local type inference: an expression has a known type only when
//! it comes from `self` in a model's method, from `env["model"]` with a
//! literal name, from `env.ref("module.xmlid")`, or from a variable or test
//! class attribute assigned one of those. Fields are followed by name; the
//! indexer later resolves each step against the models of a version. Any
//! expression whose type is not certain is left out, never guessed.

use std::collections::HashMap;

use pontix_core::{CodeRef, CodeRoot, CodeUse, ModelName};
use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};
use ruff_python_ast::{
    Comprehension, Expr, ExprAttribute, ExprCall, ExprLambda, Operator, Stmt, StmtClassDef,
    StmtFunctionDef,
};
use ruff_text_size::{Ranged, TextSize};

use super::method::is_staticmethod;
use crate::source::SourceFile;

/// Methods that return records of the same model as their receiver.
const SAME_RECORDS: [&str; 17] = [
    "browse",
    "copy",
    "create",
    // `Form`: the line being edited in a one2many.
    "edit",
    "ensure_one",
    "exists",
    "filtered",
    "filtered_domain",
    "new",
    "search",
    "sorted",
    "sudo",
    "with_company",
    "with_context",
    "with_env",
    "with_prefetch",
    "with_user",
];

/// Methods whose first argument, or `vals`, holds field values.
const WRITE_VALUES: [&str; 5] = ["copy", "create", "new", "update", "write"];

/// Methods whose first argument, or `domain`, is a domain.
const DOMAIN_ARGUMENT: [&str; 8] = [
    "_read_group",
    "_search",
    "filtered_domain",
    "formatted_read_group",
    "read_group",
    "search",
    "search_count",
    "search_read",
];

/// Methods whose string arguments are field paths.
const FIELD_STRINGS: [&str; 4] = ["filtered", "mapped", "read", "sorted"];

/// Methods whose function argument is called with each record.
const RECORD_CALLBACKS: [&str; 3] = ["filtered", "mapped", "sorted"];

/// Record attributes that are not fields.
const NOT_FIELDS: [&str; 2] = ["env", "ids"];

/// Attributes of the environment that are records.
const ENV_RECORDS: [(&str, &str); 3] = [
    ("companies", "res.company"),
    ("company", "res.company"),
    ("user", "res.users"),
];

/// What an expression is known to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Ty {
    /// Records reached from `root` through the fields in `path`.
    Records(CodeRoot, Vec<String>),
    /// An Odoo environment.
    Env,
    /// `self` or `cls` of a class that is not a model, such as a test case:
    /// its attributes are the ones its methods assign.
    Plain,
}

/// Attributes that the methods of a non-model class assign on `self` or
/// `cls`. `None` marks one assigned values of different types.
pub(super) type Attributes = HashMap<String, Option<Ty>>;

/// The kind of function being read.
#[derive(Clone, Copy)]
pub(super) enum Owner<'a> {
    /// A method of a model class: `self` is the model's records.
    Model,
    /// A method of another class, with the attributes its methods assign.
    Plain(&'a Attributes),
    /// A function outside any class.
    Function,
}

/// The references `def` makes, in source order of their expressions.
pub(super) fn refs(
    source: &SourceFile<'_>,
    def: &StmtFunctionDef,
    owner: Owner<'_>,
) -> Vec<CodeRef> {
    let mut code = Code::new(source, def, owner);
    code.bind_body(&def.body);
    code.visit_body(&def.body);
    code.out.sort_by_key(|(at, _)| *at);
    code.out.into_iter().map(|(_, found)| found).collect()
}

/// The attributes the methods of a non-model class assign on their receiver,
/// typed in source order, so `setUpClass` can build on attributes it
/// assigned before.
pub(super) fn attributes(source: &SourceFile<'_>, class: &StmtClassDef) -> Attributes {
    let mut attributes = Attributes::new();
    for stmt in &class.body {
        let Stmt::FunctionDef(def) = stmt else {
            continue;
        };
        let assigned = {
            let mut code = Code::new(source, def, Owner::Plain(&attributes));
            code.bind_body(&def.body);
            let mut assigned = Vec::new();
            code.receiver_assignments(&def.body, &mut assigned);
            assigned
        };
        for (name, ty) in assigned {
            merge(&mut attributes, name, ty);
        }
    }
    attributes
}

/// Records `ty` for `name`; a name given two different types has none.
fn merge(types: &mut HashMap<String, Option<Ty>>, name: String, ty: Option<Ty>) {
    match types.get(&name) {
        None => {
            types.insert(name, ty);
        }
        Some(known) if *known != ty => {
            types.insert(name, None);
        }
        Some(_) => {}
    }
}

/// One function being read.
struct Code<'a, 's> {
    source: &'a SourceFile<'s>,
    owner: Owner<'a>,
    /// Name of `self` or `cls`, when the function has a receiver.
    receiver: Option<String>,
    /// Local variables, flow-insensitive.
    vars: HashMap<String, Option<Ty>>,
    /// Lambda parameters and comprehension targets in scope, innermost last.
    scopes: Vec<(String, Option<Ty>)>,
    /// What was found, with where it starts.
    out: Vec<(TextSize, CodeRef)>,
}

impl<'a, 's> Code<'a, 's> {
    fn new(source: &'a SourceFile<'s>, def: &StmtFunctionDef, owner: Owner<'a>) -> Self {
        let receiver = match owner {
            Owner::Function => None,
            Owner::Model | Owner::Plain(_) if is_staticmethod(def) => None,
            Owner::Model | Owner::Plain(_) => def
                .parameters
                .posonlyargs
                .iter()
                .chain(&def.parameters.args)
                .next()
                .map(|parameter| parameter.parameter.name.as_str().to_string()),
        };
        Self {
            source,
            owner,
            receiver,
            vars: HashMap::new(),
            scopes: Vec::new(),
            out: Vec::new(),
        }
    }

    /// Types the variables the body assigns, in statement order. Functions
    /// and classes nested in it are not entered: their names are their own.
    fn bind_body(&mut self, body: &[Stmt]) {
        for stmt in body {
            match stmt {
                Stmt::Assign(assign) => {
                    let ty = self.ty(&assign.value);
                    for target in &assign.targets {
                        self.bind_target(target, ty.clone());
                    }
                }
                Stmt::AnnAssign(assign) => {
                    let ty = assign.value.as_deref().and_then(|value| self.ty(value));
                    self.bind_target(&assign.target, ty);
                }
                Stmt::AugAssign(assign) => {
                    let ty = self.ty(&assign.value);
                    self.bind_target(&assign.target, ty);
                }
                Stmt::For(for_stmt) => {
                    let ty = self.records(&for_stmt.iter);
                    self.bind_target(&for_stmt.target, ty);
                    self.bind_body(&for_stmt.body);
                    self.bind_body(&for_stmt.orelse);
                }
                Stmt::While(while_stmt) => {
                    self.bind_body(&while_stmt.body);
                    self.bind_body(&while_stmt.orelse);
                }
                Stmt::If(if_stmt) => {
                    self.bind_body(&if_stmt.body);
                    for clause in &if_stmt.elif_else_clauses {
                        self.bind_body(&clause.body);
                    }
                }
                Stmt::With(with) => {
                    for item in &with.items {
                        if let Some(target) = &item.optional_vars {
                            // `with Form(record) as form` and
                            // `with form.line_ids.new() as line`.
                            let ty = self.records(&item.context_expr);
                            self.bind_target(target, ty);
                        }
                    }
                    self.bind_body(&with.body);
                }
                Stmt::Try(try_stmt) => {
                    self.bind_body(&try_stmt.body);
                    for handler in &try_stmt.handlers {
                        let ruff_python_ast::ExceptHandler::ExceptHandler(handler) = handler;
                        self.bind_body(&handler.body);
                    }
                    self.bind_body(&try_stmt.orelse);
                    self.bind_body(&try_stmt.finalbody);
                }
                Stmt::FunctionDef(def) => self.bind_name(def.name.as_str(), None),
                Stmt::ClassDef(class) => self.bind_name(class.name.as_str(), None),
                _ => {}
            }
        }
    }

    fn bind_target(&mut self, target: &Expr, ty: Option<Ty>) {
        match target {
            Expr::Name(name) => self.bind_name(name.id.as_str(), ty),
            // Unpacking gives each name an element we cannot type.
            Expr::Tuple(tuple) => tuple
                .elts
                .iter()
                .for_each(|elt| self.bind_target(elt, None)),
            Expr::List(list) => list.elts.iter().for_each(|elt| self.bind_target(elt, None)),
            Expr::Starred(starred) => self.bind_target(&starred.value, None),
            _ => {}
        }
    }

    fn bind_name(&mut self, name: &str, ty: Option<Ty>) {
        merge(&mut self.vars, name.to_string(), ty);
    }

    /// `self.x = value` and `cls.x = value` in `body`, for [`attributes`].
    fn receiver_assignments(&self, body: &[Stmt], found: &mut Vec<(String, Option<Ty>)>) {
        let Some(receiver) = &self.receiver else {
            return;
        };
        for stmt in body {
            match stmt {
                Stmt::Assign(assign) => {
                    for target in &assign.targets {
                        if let Expr::Attribute(attribute) = target
                            && let Expr::Name(name) = &*attribute.value
                            && name.id.as_str() == receiver
                        {
                            found.push((
                                attribute.attr.as_str().to_string(),
                                self.ty(&assign.value),
                            ));
                        }
                    }
                }
                Stmt::For(for_stmt) => self.receiver_assignments(&for_stmt.body, found),
                Stmt::If(if_stmt) => {
                    self.receiver_assignments(&if_stmt.body, found);
                    for clause in &if_stmt.elif_else_clauses {
                        self.receiver_assignments(&clause.body, found);
                    }
                }
                Stmt::With(with) => self.receiver_assignments(&with.body, found),
                _ => {}
            }
        }
    }

    /// The type of `expr`, when it is certain.
    fn ty(&self, expr: &Expr) -> Option<Ty> {
        match expr {
            Expr::Name(name) => self.name(name.id.as_str()),
            Expr::Attribute(attribute) => self.attribute(attribute),
            Expr::Subscript(subscript) => match self.ty(&subscript.value)? {
                Ty::Env => model_name(&subscript.slice)
                    .map(|model| Ty::Records(CodeRoot::Model(model), Vec::new())),
                records @ Ty::Records(..) => matches!(
                    &*subscript.slice,
                    Expr::NumberLiteral(_) | Expr::Slice(_) | Expr::UnaryOp(_)
                )
                .then_some(records),
                Ty::Plain => None,
            },
            Expr::Call(call) => self.call(call),
            Expr::BinOp(binop) if binop.op == Operator::BitOr => {
                let left = self.ty(&binop.left)?;
                (self.ty(&binop.right)? == left).then_some(left)
            }
            _ => None,
        }
    }

    /// Records only: what a `for` loop over `expr` gives each time.
    fn records(&self, expr: &Expr) -> Option<Ty> {
        self.ty(expr).filter(|ty| matches!(ty, Ty::Records(..)))
    }

    fn name(&self, name: &str) -> Option<Ty> {
        if let Some((_, ty)) = self.scopes.iter().rev().find(|(scoped, _)| scoped == name) {
            return ty.clone();
        }
        if let Some(ty) = self.vars.get(name) {
            return ty.clone();
        }
        if self.receiver.as_deref() == Some(name) {
            return match self.owner {
                Owner::Model => Some(Ty::Records(CodeRoot::Own, Vec::new())),
                Owner::Plain(_) => Some(Ty::Plain),
                Owner::Function => None,
            };
        }
        (name == "env").then_some(Ty::Env)
    }

    fn attribute(&self, attribute: &ExprAttribute) -> Option<Ty> {
        let attr = attribute.attr.as_str();
        match self.ty(&attribute.value)? {
            Ty::Records(root, mut path) => {
                if attr == "env" {
                    return Some(Ty::Env);
                }
                if attr == "_origin" {
                    return Some(Ty::Records(root, path));
                }
                if attr.starts_with('_') || NOT_FIELDS.contains(&attr) {
                    return None;
                }
                path.push(attr.to_string());
                Some(Ty::Records(root, path))
            }
            Ty::Env => ENV_RECORDS
                .iter()
                .find(|(name, _)| *name == attr)
                .map(|(_, model)| Ty::Records(CodeRoot::Model(ModelName::new(*model)), Vec::new())),
            Ty::Plain => {
                if attr == "env" {
                    return Some(Ty::Env);
                }
                match self.owner {
                    Owner::Plain(attributes) => attributes.get(attr).cloned().flatten(),
                    Owner::Model | Owner::Function => None,
                }
            }
        }
    }

    fn call(&self, call: &ExprCall) -> Option<Ty> {
        let Expr::Attribute(func) = &*call.func else {
            // A test `Form` reads and writes the fields of its record.
            return match &*call.func {
                Expr::Name(name) if name.id.as_str() == "Form" => {
                    self.records(call.arguments.args.first()?)
                }
                _ => None,
            };
        };
        let method = func.attr.as_str();
        match self.ty(&func.value)? {
            Ty::Env if method == "ref" => call
                .arguments
                .args
                .first()
                .and_then(literal)
                .map(|xmlid| Ty::Records(CodeRoot::Xmlid(xmlid.to_string()), Vec::new())),
            Ty::Records(root, mut path) => {
                if SAME_RECORDS.contains(&method) {
                    return Some(Ty::Records(root, path));
                }
                if method == "mapped" {
                    path.extend(split(call.arguments.args.first().and_then(literal)?));
                    return Some(Ty::Records(root, path));
                }
                None
            }
            Ty::Env | Ty::Plain => None,
        }
    }

    fn emit(&mut self, root: CodeRoot, path: Vec<String>, how: CodeUse, at: TextSize) {
        let location = self.source.location(at);
        self.out.push((
            at,
            CodeRef {
                root,
                path,
                how,
                location,
            },
        ));
    }

    /// Visits `expr`; `chained` is true when it is part of a longer chain of
    /// fields that is reported whole.
    fn expr(&mut self, expr: &Expr, chained: bool) {
        match expr {
            Expr::Attribute(attribute) => {
                let ty = self.attribute(attribute);
                if !chained
                    && let Some(Ty::Records(root, path)) = &ty
                    && !path.is_empty()
                {
                    self.emit(root.clone(), path.clone(), CodeUse::Attribute, expr.start());
                }
                let continues = matches!(ty, Some(Ty::Records(..)));
                self.expr(&attribute.value, continues);
            }
            Expr::Subscript(subscript) => {
                if let Some(Ty::Env) = self.ty(&subscript.value)
                    && let Some(model) = model_name(&subscript.slice)
                {
                    self.emit(
                        CodeRoot::Model(model),
                        Vec::new(),
                        CodeUse::Attribute,
                        expr.start(),
                    );
                }
                let keeps = matches!(self.ty(expr), Some(Ty::Records(..)));
                self.expr(&subscript.value, chained && keeps);
                self.expr(&subscript.slice, false);
            }
            Expr::Call(call) => self.call_expr(expr, call, chained),
            Expr::ListComp(comp) => self.comprehension(&comp.generators, &[&comp.elt]),
            Expr::SetComp(comp) => self.comprehension(&comp.generators, &[&comp.elt]),
            Expr::Generator(comp) => self.comprehension(&comp.generators, &[&comp.elt]),
            Expr::DictComp(comp) => {
                let mut parts: Vec<&Expr> = vec![&comp.value];
                parts.extend(comp.key.as_deref());
                self.comprehension(&comp.generators, &parts);
            }
            _ => walk_expr(self, expr),
        }
    }

    fn call_expr(&mut self, expr: &Expr, call: &ExprCall, chained: bool) {
        let Expr::Attribute(func) = &*call.func else {
            return walk_expr(self, expr);
        };
        let method = func.attr.as_str();
        let receiver = self.ty(&func.value);
        let mut handled: Vec<&Expr> = Vec::new();
        if let Some(Ty::Records(root, path)) = &receiver {
            let arguments = &call.arguments;
            if WRITE_VALUES.contains(&method) {
                let values = arguments
                    .args
                    .first()
                    .or_else(|| arguments.find_keyword("vals").map(|keyword| &keyword.value))
                    .or_else(|| {
                        arguments
                            .find_keyword("default")
                            .map(|keyword| &keyword.value)
                    });
                if let Some(values) = values {
                    self.values(values, root, path);
                }
            }
            if DOMAIN_ARGUMENT.contains(&method) {
                let domain = arguments.args.first().or_else(|| {
                    arguments
                        .find_keyword("domain")
                        .map(|keyword| &keyword.value)
                });
                if let Some(domain) = domain {
                    self.domain(domain, root, path);
                }
            }
            if FIELD_STRINGS.contains(&method) {
                for argument in &arguments.args {
                    self.field_strings(argument, root, path);
                }
            }
            if RECORD_CALLBACKS.contains(&method) {
                let callbacks = arguments
                    .args
                    .iter()
                    .chain(arguments.keywords.iter().map(|keyword| &keyword.value));
                for argument in callbacks {
                    if let Expr::Lambda(lambda) = argument {
                        self.lambda(lambda, Ty::Records(root.clone(), path.clone()));
                        handled.push(argument);
                    }
                }
            }
        }
        let keeps = matches!(receiver, Some(Ty::Records(..)))
            && (SAME_RECORDS.contains(&method) || method == "mapped");
        self.expr(&func.value, chained && keeps);
        for argument in &call.arguments.args {
            if !handled.iter().any(|done| std::ptr::eq(*done, argument)) {
                self.expr(argument, false);
            }
        }
        for keyword in &call.arguments.keywords {
            if !handled
                .iter()
                .any(|done| std::ptr::eq(*done, &keyword.value))
            {
                self.expr(&keyword.value, false);
            }
        }
    }

    fn lambda(&mut self, lambda: &ExprLambda, record: Ty) {
        let parameters = lambda.parameters.as_deref();
        let first = parameters.and_then(|parameters| {
            parameters
                .posonlyargs
                .iter()
                .chain(&parameters.args)
                .next()
                .map(|parameter| parameter.parameter.name.as_str().to_string())
        });
        let scoped = self.scopes.len();
        if let Some(first) = first {
            self.scopes.push((first, Some(record)));
        }
        self.expr(&lambda.body, false);
        self.scopes.truncate(scoped);
    }

    fn comprehension(&mut self, generators: &[Comprehension], parts: &[&Expr]) {
        let scoped = self.scopes.len();
        for generator in generators {
            self.expr(&generator.iter, false);
            let ty = self.records(&generator.iter);
            match &generator.target {
                Expr::Name(name) => self.scopes.push((name.id.as_str().to_string(), ty)),
                target => {
                    for name in names(target) {
                        self.scopes.push((name, None));
                    }
                }
            }
            for condition in &generator.ifs {
                self.expr(condition, false);
            }
        }
        for part in parts {
            self.expr(part, false);
        }
        self.scopes.truncate(scoped);
    }

    /// The keys of a values dict, or of a list of them, on records reached
    /// through `path`. One2many and many2many commands that carry values
    /// are followed into the comodel.
    fn values(&mut self, expr: &Expr, root: &CodeRoot, path: &[String]) {
        match expr {
            Expr::Dict(dict) => {
                for item in &dict.items {
                    let Some(key) = item.key.as_ref() else {
                        continue;
                    };
                    let Some(field) = literal(key) else {
                        continue;
                    };
                    let mut field_path = path.to_vec();
                    field_path.push(field.to_string());
                    self.emit(
                        root.clone(),
                        field_path.clone(),
                        CodeUse::Values,
                        key.start(),
                    );
                    for command in elements(&item.value) {
                        if let Some(values) = command_values(command) {
                            self.values(values, root, &field_path);
                        }
                    }
                }
            }
            Expr::List(_) | Expr::Tuple(_) => {
                for element in elements(expr) {
                    if matches!(element, Expr::Dict(_)) {
                        self.values(element, root, path);
                    }
                }
            }
            _ => {}
        }
    }

    /// The field paths of the conditions of a literal domain.
    fn domain(&mut self, expr: &Expr, root: &CodeRoot, path: &[String]) {
        for condition in elements(expr) {
            let parts = elements(condition);
            if parts.len() != 3 {
                continue;
            }
            if let Some(field) = literal(parts[0]) {
                let mut field_path = path.to_vec();
                field_path.extend(split(field));
                self.emit(root.clone(), field_path, CodeUse::Domain, parts[0].start());
            }
        }
    }

    /// A field path given as a string, or a list of them, as in
    /// `read(["name", "email"])`.
    fn field_strings(&mut self, expr: &Expr, root: &CodeRoot, path: &[String]) {
        let strings: Vec<&Expr> = match expr {
            Expr::StringLiteral(_) => vec![expr],
            Expr::List(_) | Expr::Tuple(_) => elements(expr),
            _ => Vec::new(),
        };
        for string in strings {
            if let Some(field) = literal(string) {
                let mut field_path = path.to_vec();
                field_path.extend(split(field));
                self.emit(
                    root.clone(),
                    field_path,
                    CodeUse::FieldString,
                    string.start(),
                );
            }
        }
    }
}

impl<'e> Visitor<'e> for Code<'_, '_> {
    fn visit_stmt(&mut self, stmt: &'e Stmt) {
        match stmt {
            // A nested class has a body of its own.
            Stmt::ClassDef(_) => {}
            other => walk_stmt(self, other),
        }
    }

    fn visit_expr(&mut self, expr: &'e Expr) {
        self.expr(expr, false);
    }
}

/// The values a one2many or many2many command carries: `Command.create(v)`,
/// `Command.update(id, v)`, `(0, 0, v)` and `(1, id, v)`.
fn command_values(command: &Expr) -> Option<&Expr> {
    match command {
        Expr::Call(call) => {
            let Expr::Attribute(func) = &*call.func else {
                return None;
            };
            let is_command = match &*func.value {
                Expr::Name(name) => name.id.as_str() == "Command",
                Expr::Attribute(attribute) => attribute.attr.as_str() == "Command",
                _ => false,
            };
            if !is_command {
                return None;
            }
            match func.attr.as_str() {
                "create" => call.arguments.args.first(),
                "update" => call.arguments.args.get(1),
                _ => None,
            }
        }
        Expr::Tuple(_) | Expr::List(_) => {
            let parts = elements(command);
            let Expr::NumberLiteral(code) = parts.first()? else {
                return None;
            };
            let code = code.value.as_int()?.as_u8()?;
            (matches!(code, 0 | 1) && parts.len() == 3).then(|| parts[2])
        }
        _ => None,
    }
}

/// The elements of a list or tuple literal; nothing for anything else.
fn elements(expr: &Expr) -> Vec<&Expr> {
    match expr {
        Expr::List(list) => list.elts.iter().collect(),
        Expr::Tuple(tuple) => tuple.elts.iter().collect(),
        _ => Vec::new(),
    }
}

/// Names bound by an unpacking target.
fn names(target: &Expr) -> Vec<String> {
    match target {
        Expr::Name(name) => vec![name.id.as_str().to_string()],
        Expr::Tuple(_) | Expr::List(_) => elements(target).into_iter().flat_map(names).collect(),
        Expr::Starred(starred) => names(&starred.value),
        _ => Vec::new(),
    }
}

fn literal(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::StringLiteral(literal) => Some(literal.value.to_str()),
        _ => None,
    }
}

fn model_name(expr: &Expr) -> Option<ModelName> {
    literal(expr)
        .filter(|name| !name.is_empty())
        .map(ModelName::new)
}

fn split(path: &str) -> impl Iterator<Item = String> + '_ {
    path.split('.')
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use pontix_core::{CodeRef, CodeRoot, CodeUse};

    use crate::python::parse_python;

    /// Each reference as `root:path how line`, for easy comparison.
    fn show(refs: &[CodeRef]) -> Vec<String> {
        refs.iter()
            .map(|r| {
                let root = match &r.root {
                    CodeRoot::Own => "self".to_string(),
                    CodeRoot::Model(model) => model.to_string(),
                    CodeRoot::Xmlid(xmlid) => format!("ref({xmlid})"),
                };
                let how = match r.how {
                    CodeUse::Attribute => "attr",
                    CodeUse::Values => "vals",
                    CodeUse::Domain => "domain",
                    CodeUse::FieldString => "string",
                };
                format!("{root}:{} {how} {}", r.path.join("."), r.location.line)
            })
            .collect()
    }

    fn model_refs(lines: &[&str]) -> Vec<String> {
        let file = parse_python(Path::new("models/x.py"), &lines.join("\n"));
        let refs: Vec<CodeRef> = file
            .models
            .iter()
            .flat_map(|model| &model.methods)
            .flat_map(|method| method.code_refs.clone())
            .collect();
        show(&refs)
    }

    fn other_refs(lines: &[&str]) -> Vec<String> {
        show(&parse_python(Path::new("tests/test_x.py"), &lines.join("\n")).code_refs)
    }

    #[test]
    fn follows_self_and_reports_each_chain_once() {
        let refs = model_refs(&[
            "from odoo import models",
            "class SaleOrder(models.Model):",
            "    _inherit = 'sale.order'",
            "    def _compute(self):",
            "        for order in self:",
            "            order.x_total = order.order_line.sudo().product_uom.name",
            "            order.partner_id.write({'email': False})",
            "            self._context.get('lang')",
            "            order.action_confirm()",
        ]);
        assert_eq!(
            refs,
            [
                "self:x_total attr 6",
                "self:order_line.product_uom.name attr 6",
                "self:partner_id attr 7",
                "self:partner_id.email vals 7",
            ]
        );
    }

    #[test]
    fn env_models_values_domains_and_strings() {
        let refs = model_refs(&[
            "from odoo import Command, models",
            "class Picking(models.Model):",
            "    _inherit = 'stock.picking'",
            "    def run(self):",
            "        packs = self.env['stock.quant.package'].search([('name', '=', 'x'), '|', ('location_id.usage', '=', 'internal')])",
            "        lines = self.move_ids.mapped('move_line_ids')",
            "        lines.filtered(lambda l: l.result_package_id)",
            "        self.env['sale.order'].create({'order_line': [Command.create({'product_uom': 1}), (0, 0, {'tax_id': []})]})",
            "        self.read(['state', 'note'])",
            "        self.env.user.company_id",
            "        self.env.ref('base.main_company').partner_id",
        ]);
        assert_eq!(
            refs,
            [
                "stock.quant.package: attr 5",
                "stock.quant.package:name domain 5",
                "stock.quant.package:location_id.usage domain 5",
                "self:move_ids attr 6",
                "self:move_ids.move_line_ids string 6",
                "self:move_ids.move_line_ids.result_package_id attr 7",
                "sale.order: attr 8",
                "sale.order:order_line vals 8",
                "sale.order:order_line.product_uom vals 8",
                "sale.order:order_line.tax_id vals 8",
                "self:state string 9",
                "self:note string 9",
                "res.users:company_id attr 10",
                "ref(base.main_company):partner_id attr 11",
            ]
        );
    }

    #[test]
    fn variables_with_two_types_and_parameters_are_unknown() {
        let refs = model_refs(&[
            "from odoo import models",
            "class Move(models.Model):",
            "    _inherit = 'account.move'",
            "    def run(self, other):",
            "        rec = self.partner_id",
            "        if other:",
            "            rec = self.company_id",
            "        rec.name",
            "        other.name",
            "        x, y = self, self",
            "        x.name",
        ]);
        assert_eq!(refs, ["self:partner_id attr 5", "self:company_id attr 7"]);
    }

    #[test]
    fn test_cases_type_the_attributes_their_setup_assigns() {
        let refs = other_refs(&[
            "from odoo.tests import TransactionCase",
            "class TestX(TransactionCase):",
            "    @classmethod",
            "    def setUpClass(cls):",
            "        super().setUpClass()",
            "        cls.partner = cls.env['res.partner'].create({'name': 'A'})",
            "        cls.order = cls.env['sale.order'].create({'partner_id': cls.partner.id})",
            "    def test_one(self):",
            "        self.order.order_line.product_uom",
            "        self.partner.write({'groups_id': []})",
            "        self.unknown.name",
            "        with Form(self.env['purchase.order']) as po:",
            "            with po.order_line.new() as line:",
            "                line.taxes_id.clear()",
        ]);
        assert_eq!(
            refs,
            [
                "res.partner: attr 6",
                "res.partner:name vals 6",
                "sale.order: attr 7",
                "sale.order:partner_id vals 7",
                "res.partner:id attr 7",
                "sale.order:order_line.product_uom attr 9",
                "res.partner:groups_id vals 10",
                "purchase.order: attr 12",
                "purchase.order:order_line attr 13",
                "purchase.order:order_line.taxes_id attr 14",
            ]
        );
    }

    #[test]
    fn functions_use_env_and_never_type_self() {
        let refs = other_refs(&[
            "def post_init_hook(env):",
            "    env['res.partner'].search([('ref', '!=', False)]).write({'ref': False})",
            "class Helper:",
            "    def run(self):",
            "        self.partner_id.name",
        ]);
        assert_eq!(
            refs,
            [
                "res.partner: attr 2",
                "res.partner:ref domain 2",
                "res.partner:ref vals 2",
            ]
        );
    }
}
