//! The fields QWeb reports read: `t-field="doc.partner_id"`,
//! `t-foreach="doc.order_line" t-as="line"`, `t-esc="line.price_unit"`.
//!
//! A report renders its template with `docs`, records of the report's
//! model. Pontix follows the template as the source version renders it
//! (with every extension in the module's scope applied), through `t-call`,
//! knowing which variable holds records of which model, and checks the
//! fields the module's own elements read. A variable it cannot type, such
//! as one a method returns, is left alone, and only fields the upgrade
//! removes are reported: the source version renders the report, so a name
//! it does not know is one Pontix cannot see.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use pontix_core::{Element, Location, ModelName, ModuleName};
use pontix_index::Module;
use pontix_index::registry::qualified;
use pontix_parsers::{Chain, format_expressions, read_expression};

use super::Checker;
use crate::PathUse;
use crate::path::reach;

/// Deepest `t-call` nesting followed.
const MAX_CALLS: usize = 12;

/// Variables bound to records, by name.
type Variables = BTreeMap<String, ModelName>;

/// A field path a module's element reads.
struct Read {
    model: ModelName,
    path: String,
    template: String,
    attribute: String,
    at: Location,
}

impl Checker<'_, '_, '_, '_> {
    /// The fields the module's QWeb elements read in the reports of its
    /// scope that render them.
    pub(super) fn qweb(&mut self, module: &Module) {
        if !module.data.iter().any(|(_, file)| has_templates(file)) {
            return;
        }
        let source = &self.versions.source;
        let from = self.scope.0;
        let in_scope = |name: &ModuleName| from.has(name);
        let mut reads = Vec::new();
        let mut walked = BTreeSet::new();
        for found in &source.full.modules {
            if !from.has(&found.name) {
                continue;
            }
            for (_, file) in &found.data {
                for report in &file.reports {
                    let template = qualified(&found.name, &report.template);
                    let variables = Variables::from([
                        ("docs".to_string(), report.model.clone()),
                        ("user".to_string(), ModelName::new("res.users")),
                        ("res_company".to_string(), ModelName::new("res.company")),
                    ]);
                    let mut walker = Walker {
                        checker: self,
                        in_scope: &in_scope,
                        module: &module.path,
                        walked: &mut walked,
                        reads: &mut reads,
                    };
                    walker.call(&template, variables, 0);
                }
            }
        }
        let mut seen = BTreeSet::new();
        for read in reads {
            let key = (
                read.model.clone(),
                read.path.clone(),
                read.at.file.clone(),
                read.at.line,
            );
            if !seen.insert(key) {
                continue;
            }
            let how = PathUse::Qweb(read.template, read.attribute);
            self.path(&read.model, &read.path, how, &read.at);
        }
    }
}

/// Whether a data file declares QWeb: a report, or a view whose
/// architecture has a `t-` directive.
fn has_templates(file: &pontix_core::DataFile) -> bool {
    fn directive(element: &Element) -> bool {
        element
            .attributes
            .iter()
            .any(|(name, _)| name.starts_with("t-"))
            || element.children.iter().any(directive)
    }
    !file.reports.is_empty()
        || file.records.iter().any(|record| {
            record
                .view
                .as_ref()
                .and_then(|view| view.arch.as_ref())
                .is_some_and(directive)
        })
}

struct Walker<'w, 'c, 'v, 's, 'i, 'a> {
    checker: &'w Checker<'c, 's, 'i, 'a>,
    in_scope: &'w dyn Fn(&ModuleName) -> bool,
    module: &'w Path,
    /// Templates already followed with the same variables.
    walked: &'w mut BTreeSet<(String, Vec<(String, ModelName)>)>,
    reads: &'v mut Vec<Read>,
}

impl Walker<'_, '_, '_, '_, '_, '_> {
    /// Renders a template with these variables.
    fn call(&mut self, template: &str, variables: Variables, depth: usize) {
        if depth > MAX_CALLS {
            return;
        }
        let key = (
            template.to_string(),
            variables
                .iter()
                .map(|(name, model)| (name.clone(), model.clone()))
                .collect(),
        );
        if !self.walked.insert(key) {
            return;
        }
        let Some(arch) = self
            .checker
            .versions
            .source
            .views
            .arch(template, self.in_scope)
        else {
            return;
        };
        let mut variables = variables;
        self.children(&arch, template, &mut variables, depth);
    }

    /// The children of an element, in order: a `t-set` binds for those
    /// after it.
    fn children(
        &mut self,
        element: &Element,
        template: &str,
        variables: &mut Variables,
        depth: usize,
    ) {
        for child in &element.children {
            self.element(child, template, variables, depth);
        }
    }

    fn element(
        &mut self,
        element: &Element,
        template: &str,
        variables: &mut Variables,
        depth: usize,
    ) {
        // `t-foreach` first: the rest of the element runs once per item.
        if let Some(each) = element.attribute("t-foreach") {
            self.read(element, template, "t-foreach", each, variables);
            let mut inner = variables.clone();
            if let Some(name) = element.attribute("t-as") {
                match self.model_of(each, variables) {
                    Some(model) => inner.insert(name.to_string(), model),
                    None => inner.remove(name),
                };
            }
            self.directives(element, template, &mut inner, depth);
            return;
        }
        self.directives(element, template, variables, depth);
    }

    fn directives(
        &mut self,
        element: &Element,
        template: &str,
        variables: &mut Variables,
        depth: usize,
    ) {
        for (name, value) in &element.attributes {
            match name.as_str() {
                "t-if" | "t-elif" | "t-esc" | "t-out" | "t-raw" | "t-field" | "t-value"
                | "t-options" => self.read(element, template, name, value, variables),
                attribute if attribute.starts_with("t-att-") => {
                    self.read(element, template, name, value, variables);
                }
                attribute if attribute.starts_with("t-attf-") => {
                    for piece in format_expressions(value) {
                        self.read(element, template, name, piece, variables);
                    }
                }
                _ => {}
            }
        }
        if let Some(set) = element.attribute("t-set") {
            // The value form binds records; the body form binds markup.
            let model = element
                .attribute("t-value")
                .and_then(|value| self.model_of(value, variables));
            match model {
                Some(model) => variables.insert(set.to_string(), model),
                None => variables.remove(set),
            };
            return;
        }
        if let Some(called) = element.attribute("t-call") {
            // The body's `t-set` pass variables to the template called.
            let mut passed = variables.clone();
            self.children(element, template, &mut passed, depth);
            if !called.contains("{{") && !called.contains("#{") {
                let called = match called.split_once('.') {
                    Some(_) => called.to_string(),
                    None => match template.split_once('.') {
                        Some((owner, _)) => qualified(&ModuleName::new(owner), called),
                        None => called.to_string(),
                    },
                };
                self.call(&called, passed, depth + 1);
            }
            return;
        }
        let mut inner = variables.clone();
        self.children(element, template, &mut inner, depth);
    }

    /// The model of the records an expression gives, when it is a chain of
    /// fields from a variable bound to records.
    fn model_of(&self, expression: &str, variables: &Variables) -> Option<ModelName> {
        let chain = read_expression(expression)?.value?;
        let model = variables.get(&chain.root)?;
        let fields = self.fields(&chain)?;
        let source = &self.checker.versions.source;
        reach(&source.models, self.checker.scope.0, model, &fields)
    }

    /// The fields a chain follows from its variable: up to a member every
    /// model has (`env`, `ids`, `mapped`) or a private attribute, which
    /// are not fields. `None` when it follows none.
    fn fields(&self, chain: &Chain) -> Option<Vec<String>> {
        let framework = self.checker.versions.source.framework;
        let fields: Vec<String> = chain
            .attributes
            .iter()
            .take_while(|attribute| {
                !attribute.starts_with('_') && framework.base_model(attribute).is_none()
            })
            .cloned()
            .collect();
        Some(fields)
    }

    /// The field paths an expression of one of the module's elements reads.
    fn read(
        &mut self,
        element: &Element,
        template: &str,
        attribute: &str,
        expression: &str,
        variables: &Variables,
    ) {
        if !element.location.file.starts_with(self.module) {
            return;
        }
        let Some(read) = read_expression(expression) else {
            return;
        };
        for chain in &read.chains {
            let Some(model) = variables.get(&chain.root) else {
                continue;
            };
            let Some(fields) = self.fields(chain).filter(|fields| !fields.is_empty()) else {
                continue;
            };
            self.reads.push(Read {
                model: model.clone(),
                path: fields.join("."),
                template: template.to_string(),
                attribute: attribute.to_string(),
                at: element.location.clone(),
            });
        }
    }
}
