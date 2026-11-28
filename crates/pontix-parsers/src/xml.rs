//! Odoo XML data files, read as Odoo's `convert.py` would see them: records
//! with their external ids, view architectures, and references to records by
//! external id.
//!
//! Inputs are untrusted. A DTD is refused, so no entity is ever expanded or
//! fetched; the number of nodes and the depth Pontix follows are capped.

use std::path::Path;

use pontix_core::{
    ArchField, DataFile, Diagnostic, DiagnosticKind, FieldName, Location, ModelName, Record,
    RefKind, View, XmlRef,
};
use roxmltree::{Document, Node, ParsingOptions};
use ruff_python_ast::Expr;
use ruff_python_parser::parse_expression;
use ruff_text_size::TextSize;

use crate::source::SourceFile;

/// Most nodes Pontix reads from one file. Odoo's largest data files hold a
/// few hundred thousand.
const NODES_LIMIT: u32 = 2_000_000;

/// Deepest nesting Pontix follows. Real views nest a few dozen levels.
const MAX_DEPTH: usize = 256;

/// Elements that open an embedded view inside an x2many field.
const SUBVIEWS: [&str; 12] = [
    "list",
    "tree",
    "form",
    "kanban",
    "graph",
    "pivot",
    "calendar",
    "search",
    "activity",
    "gantt",
    "map",
    "hierarchy",
];

/// What one XML data file declares, and what could not be read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct XmlFile {
    /// Records, views and references.
    pub data: DataFile,
    /// What could not be read.
    pub diagnostics: Vec<Diagnostic>,
}

/// Reads an XML data file. A file that is not well-formed is reported and
/// yields nothing, as Odoo would refuse to load it.
pub fn parse_xml(file: &Path, text: &str) -> XmlFile {
    let source = SourceFile::new(file, text);
    let options = ParsingOptions {
        allow_dtd: false,
        nodes_limit: NODES_LIMIT,
        ..ParsingOptions::default()
    };
    let document = match Document::parse_with_options(text, options) {
        Ok(document) => document,
        Err(error) => {
            let line =
                std::num::NonZeroU32::new(error.pos().row).unwrap_or(std::num::NonZeroU32::MIN);
            return XmlFile {
                data: DataFile::default(),
                diagnostics: vec![Diagnostic {
                    kind: DiagnosticKind::InvalidXml,
                    location: Location::new(file, line),
                    snippet: error.to_string(),
                }],
            };
        }
    };
    let mut reader = Reader {
        source: &source,
        out: XmlFile::default(),
        too_deep: false,
    };
    let root = document.root_element();
    if matches!(root.tag_name().name(), "odoo" | "openerp") {
        reader.refs(root);
        reader.container(root, 0);
    }
    reader.out
}

struct Reader<'s> {
    source: &'s SourceFile<'s>,
    out: XmlFile,
    too_deep: bool,
}

impl Reader<'_> {
    fn location(&self, node: Node<'_, '_>) -> Location {
        // Offsets past u32::MAX cannot happen: files over 10 MiB are not read.
        let offset = u32::try_from(node.range().start).unwrap_or(u32::MAX);
        self.source.location(TextSize::new(offset))
    }

    fn deep(&mut self, node: Node<'_, '_>, depth: usize) -> bool {
        if depth <= MAX_DEPTH {
            return false;
        }
        if !self.too_deep {
            self.too_deep = true;
            self.out.diagnostics.push(Diagnostic {
                kind: DiagnosticKind::XmlTooDeep,
                location: self.location(node),
                snippet: node.tag_name().name().to_string(),
            });
        }
        true
    }

    /// `<odoo>`, `<openerp>` or `<data>`: the elements Odoo loads.
    fn container(&mut self, node: Node<'_, '_>, depth: usize) {
        if self.deep(node, depth) {
            return;
        }
        for child in node.children().filter(Node::is_element) {
            match child.tag_name().name() {
                "data" => self.container(child, depth + 1),
                "record" => self.record(child, depth + 1),
                "template" => {
                    self.out.data.records.push(Record {
                        id: child.attribute("id").map(str::to_string),
                        model: ModelName::new("ir.ui.view"),
                        fields: Vec::new(),
                        view: Some(View {
                            model: None,
                            inherit_id: child.attribute("inherit_id").map(str::to_string),
                            fields: Vec::new(),
                        }),
                        location: self.location(child),
                    });
                }
                "menuitem" => self.menuitem(child, depth + 1),
                "function" | "delete" => {
                    if let Some(model) = child.attribute("model") {
                        let location = self.location(child);
                        self.out.data.models.push((ModelName::new(model), location));
                    }
                    if child.has_tag_name("function")
                        && child.attribute("model") == Some("ir.model.data")
                        && child.attribute("name") == Some("_update_xmlids")
                    {
                        self.update_xmlids(child);
                    }
                }
                _ => {}
            }
        }
    }

    /// `<function model="ir.model.data" name="_update_xmlids">`: each
    /// `<value eval="[{'xml_id': ..., 'record': ...}]">` names external ids
    /// for records made elsewhere, such as a warehouse's locations.
    fn update_xmlids(&mut self, node: Node<'_, '_>) {
        for value in node.children().filter(|child| child.has_tag_name("value")) {
            let Some(eval) = value.attribute("eval") else {
                continue;
            };
            let location = self.location(value);
            for xmlid in eval_xmlids(eval) {
                self.out.data.xmlids.push((xmlid, location.clone()));
            }
        }
    }

    fn menuitem(&mut self, node: Node<'_, '_>, depth: usize) {
        if self.deep(node, depth) {
            return;
        }
        self.out.data.records.push(Record {
            id: node.attribute("id").map(str::to_string),
            model: ModelName::new("ir.ui.menu"),
            fields: Vec::new(),
            view: None,
            location: self.location(node),
        });
        // A menu written inside another has it as parent.
        for child in node
            .children()
            .filter(|child| child.has_tag_name("menuitem"))
        {
            self.menuitem(child, depth + 1);
        }
    }

    fn record(&mut self, node: Node<'_, '_>, depth: usize) {
        let Some(model) = node.attribute("model") else {
            return;
        };
        let model = ModelName::new(model);
        let mut fields = Vec::new();
        let mut view = (model.as_str() == "ir.ui.view").then(View::default);
        for field in node.children().filter(|child| child.has_tag_name("field")) {
            let Some(name) = field.attribute("name") else {
                continue;
            };
            fields.push((FieldName::new(name), self.location(field)));
            let Some(view) = view.as_mut() else {
                continue;
            };
            match name {
                "model" => view.model = field.text().map(|text| ModelName::new(text.trim())),
                "inherit_id" => view.inherit_id = field.attribute("ref").map(str::to_string),
                "arch" => view.fields = self.arch(field, depth + 1),
                _ => {}
            }
        }
        self.out.data.records.push(Record {
            id: node.attribute("id").map(str::to_string),
            model,
            fields,
            view,
            location: self.location(node),
        });
    }

    /// The `<field>` elements of a view architecture, each with the x2many
    /// fields whose embedded views hold it.
    fn arch(&mut self, arch: Node<'_, '_>, depth: usize) -> Vec<ArchField> {
        let mut found = Vec::new();
        let mut pending: Vec<(Node<'_, '_>, Vec<FieldName>, usize)> = arch
            .children()
            .filter(Node::is_element)
            .map(|child| (child, Vec::new(), depth + 1))
            .rev()
            .collect();
        while let Some((node, within, depth)) = pending.pop() {
            if self.deep(node, depth) {
                continue;
            }
            let inner = match node.tag_name().name() {
                "field" => match node.attribute("name") {
                    Some(name) => {
                        let name = FieldName::new(name);
                        let position = node.attribute("position");
                        found.push(ArchField {
                            name: name.clone(),
                            within: within.clone(),
                            locator: position.is_some(),
                            location: self.location(node),
                        });
                        // Elements inside a field are its embedded views,
                        // unless the field only locates where they go.
                        if position.is_none() || position == Some("inside") {
                            let mut inner = within.clone();
                            inner.push(name);
                            inner
                        } else {
                            within.clone()
                        }
                    }
                    None => within.clone(),
                },
                "xpath" => {
                    let mut inner = within.clone();
                    inner.extend(xpath_within(node.attribute("expr").unwrap_or_default()));
                    inner
                }
                "attribute" => continue,
                _ => within.clone(),
            };
            let children: Vec<_> = node.children().filter(Node::is_element).collect();
            for child in children.into_iter().rev() {
                pending.push((child, inner.clone(), depth + 1));
            }
        }
        found
    }

    /// Every reference by external id in the file, wherever it is.
    fn refs(&mut self, root: Node<'_, '_>) {
        let mut pending = vec![(root, 0)];
        while let Some((node, depth)) = pending.pop() {
            if self.deep(node, depth) {
                continue;
            }
            let location = self.location(node);
            let mut add = |xmlid: &str, kind| {
                let xmlid = xmlid.trim();
                if !xmlid.is_empty() {
                    self.out.data.refs.push(XmlRef {
                        xmlid: xmlid.to_string(),
                        kind,
                        location: location.clone(),
                    });
                }
            };
            let tag = node.tag_name().name();
            for attribute in node.attributes() {
                let value = attribute.value();
                match (tag, attribute.name()) {
                    ("field", "ref") if node.attribute("name") == Some("inherit_id") => {
                        add(value, RefKind::InheritId);
                    }
                    ("field", "ref") => add(value, RefKind::Ref),
                    ("template", "inherit_id") => add(value, RefKind::InheritId),
                    ("menuitem", "action") => add(value, RefKind::Action),
                    ("menuitem", "parent") => add(value, RefKind::Parent),
                    (_, "groups") => {
                        for group in value.split(',') {
                            add(group.trim().trim_start_matches('!'), RefKind::Groups);
                        }
                    }
                    (_, "eval") => {
                        for xmlid in eval_refs(value) {
                            add(xmlid, RefKind::Eval);
                        }
                    }
                    _ => {
                        for xmlid in percent_refs(value) {
                            add(xmlid, RefKind::Action);
                        }
                    }
                }
            }
            let children: Vec<_> = node.children().filter(Node::is_element).collect();
            for child in children.into_iter().rev() {
                pending.push((child, depth + 1));
            }
        }
    }
}

/// The x2many fields an XPath expression goes into, outermost first:
/// `//field[@name='order_line']/list/field[@name='name']` goes into the
/// embedded list of `order_line`.
fn xpath_within(expr: &str) -> Vec<FieldName> {
    let mut within = Vec::new();
    let mut last = None;
    for step in expr
        .split('/')
        .map(str::trim)
        .filter(|step| !step.is_empty())
    {
        if let Some(name) = field_step(step) {
            last = Some(name);
            continue;
        }
        let tag = step.split(['[', '(']).next().unwrap_or_default();
        if SUBVIEWS.contains(&tag)
            && let Some(field) = last.take()
        {
            within.push(field);
        }
    }
    within
}

/// The name in a step such as `field[@name='order_line']`.
fn field_step(step: &str) -> Option<FieldName> {
    let rest = step.strip_prefix("field[")?;
    let start = rest.find("@name=")? + "@name=".len();
    let quoted = &rest[start..];
    let quote = quoted.chars().next().filter(|c| *c == '\'' || *c == '"')?;
    let inner = &quoted[1..];
    let end = inner.find(quote)?;
    Some(FieldName::new(&inner[..end]))
}

/// The external ids of `ref('...')` calls in an `eval` expression.
fn eval_refs(expr: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = expr;
    while let Some(at) = rest.find("ref(") {
        let preceded = rest[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.');
        let after = rest[at + 4..].trim_start();
        rest = &rest[at + 4..];
        if preceded {
            continue;
        }
        let Some(quote) = after.chars().next().filter(|c| *c == '\'' || *c == '"') else {
            continue;
        };
        if let Some(end) = after[1..].find(quote) {
            found.push(&after[1..=end]);
        }
    }
    found
}

/// The literal `'xml_id'` values of the dictionaries in an `eval` list,
/// as `_update_xmlids` takes them. Anything else, or Python that does not
/// parse, yields nothing.
fn eval_xmlids(expr: &str) -> Vec<String> {
    let Ok(parsed) = parse_expression(expr.trim()) else {
        return Vec::new();
    };
    let Expr::List(list) = &*parsed.syntax().body else {
        return Vec::new();
    };
    list.elts
        .iter()
        .filter_map(|element| match element {
            Expr::Dict(dict) => dict.items.iter().find_map(|item| {
                let key = match &item.key {
                    Some(Expr::StringLiteral(key)) => key.value.to_str(),
                    _ => return None,
                };
                match &item.value {
                    Expr::StringLiteral(value) if key == "xml_id" => {
                        Some(value.value.to_str().trim().to_string())
                    }
                    _ => None,
                }
            }),
            _ => None,
        })
        .filter(|xmlid| !xmlid.is_empty())
        .collect()
}

/// The external ids of `%(...)d` and `%(...)s` in an attribute, as in a
/// button's `name="%(sale.action_view_sale_advance_payment_inv)d"`.
fn percent_refs(value: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = value;
    while let Some(start) = rest.find("%(") {
        let after = &rest[start + 2..];
        let Some(end) = after.find(')') else {
            break;
        };
        if after[end + 1..].starts_with(['d', 's']) {
            found.push(&after[..end]);
        }
        rest = &after[end..];
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(lines: &[&str]) -> XmlFile {
        parse_xml(Path::new("views/test.xml"), &lines.join("\n"))
    }

    fn names(fields: &[ArchField]) -> Vec<(String, Vec<String>, bool)> {
        fields
            .iter()
            .map(|field| {
                (
                    field.name.to_string(),
                    field.within.iter().map(ToString::to_string).collect(),
                    field.locator,
                )
            })
            .collect()
    }

    #[test]
    fn reads_records_views_and_their_fields() {
        let file = parse(&[
            "<?xml version='1.0' encoding='utf-8'?>",
            "<odoo>",
            "  <data noupdate='1'>",
            "    <record id='view_order_form' model='ir.ui.view'>",
            "      <field name='name'>sale.order.form</field>",
            "      <field name='model'>sale.order</field>",
            "      <field name='arch' type='xml'>",
            "        <form>",
            "          <field name='partner_id'/>",
            "          <field name='order_line'>",
            "            <list><field name='product_id'/></list>",
            "          </field>",
            "        </form>",
            "      </field>",
            "    </record>",
            "  </data>",
            "  <record id='tag_vip' model='res.partner.category'>",
            "    <field name='name'>VIP</field>",
            "  </record>",
            "</odoo>",
        ]);
        assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
        let records = &file.data.records;
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].id.as_deref(), Some("view_order_form"));
        assert_eq!(records[0].location.line.get(), 4);
        let view = records[0].view.as_ref().expect("a view record");
        assert_eq!(view.model, Some(ModelName::new("sale.order")));
        assert_eq!(
            names(&view.fields),
            [
                ("partner_id".to_string(), vec![], false),
                ("order_line".to_string(), vec![], false),
                (
                    "product_id".to_string(),
                    vec!["order_line".to_string()],
                    false
                ),
            ]
        );
        assert_eq!(records[1].model, ModelName::new("res.partner.category"));
        assert_eq!(records[1].fields[0].0, FieldName::new("name"));
    }

    #[test]
    fn inherited_views_locate_fields_by_xpath_and_by_field() {
        let file = parse(&[
            "<odoo>",
            "  <record id='view_order_form_custom' model='ir.ui.view'>",
            "    <field name='model'>sale.order</field>",
            "    <field name='inherit_id' ref='sale.view_order_form'/>",
            "    <field name='arch' type='xml'>",
            "      <field name='partner_id' position='after'>",
            "        <field name='customer_ref'/>",
            "      </field>",
            "      <xpath expr=\"//field[@name='order_line']/list/field[@name='name']\" position='after'>",
            "        <field name='line_ref'/>",
            "        <attribute name='invisible'>1</attribute>",
            "      </xpath>",
            "    </field>",
            "  </record>",
            "</odoo>",
        ]);
        let view = file.data.records[0].view.as_ref().expect("a view record");
        assert_eq!(view.inherit_id.as_deref(), Some("sale.view_order_form"));
        assert_eq!(
            names(&view.fields),
            [
                ("partner_id".to_string(), vec![], true),
                ("customer_ref".to_string(), vec![], false),
                (
                    "line_ref".to_string(),
                    vec!["order_line".to_string()],
                    false
                ),
            ]
        );
        let refs: Vec<(&str, RefKind)> = file
            .data
            .refs
            .iter()
            .map(|r| (r.xmlid.as_str(), r.kind))
            .collect();
        assert_eq!(refs, [("sale.view_order_form", RefKind::InheritId)]);
    }

    #[test]
    fn finds_every_kind_of_reference() {
        let file = parse(&[
            "<odoo>",
            "  <record id='action_rule' model='base.automation'>",
            "    <field name='model_id' ref='sale.model_sale_order'/>",
            "    <field name='group_ids' eval=\"[(4, ref('base.group_user')), (4, ref(&quot;sales_team.group_sale_manager&quot;))]\"/>",
            "  </record>",
            "  <menuitem id='menu_custom' action='action_custom' parent='sale.sale_menu_root' groups='base.group_user,!base.group_portal'>",
            "    <menuitem id='menu_child' action='sale.action_orders'/>",
            "  </menuitem>",
            "  <template id='report_custom' inherit_id='sale.report_saleorder_document'>",
            "    <button name='%(sale.action_view_sale_advance_payment_inv)d' type='action'/>",
            "  </template>",
            "  <function model='res.partner' name='_install_hook'/>",
            "</odoo>",
        ]);
        let refs: Vec<(&str, RefKind)> = file
            .data
            .refs
            .iter()
            .map(|r| (r.xmlid.as_str(), r.kind))
            .collect();
        assert_eq!(
            refs,
            [
                ("sale.model_sale_order", RefKind::Ref),
                ("base.group_user", RefKind::Eval),
                ("sales_team.group_sale_manager", RefKind::Eval),
                ("action_custom", RefKind::Action),
                ("sale.sale_menu_root", RefKind::Parent),
                ("base.group_user", RefKind::Groups),
                ("base.group_portal", RefKind::Groups),
                ("sale.action_orders", RefKind::Action),
                ("sale.report_saleorder_document", RefKind::InheritId),
                ("sale.action_view_sale_advance_payment_inv", RefKind::Action),
            ]
        );
        let menus: Vec<&str> = file
            .data
            .records
            .iter()
            .filter_map(|record| record.id.as_deref())
            .collect();
        assert_eq!(
            menus,
            ["action_rule", "menu_custom", "menu_child", "report_custom"]
        );
        assert_eq!(file.data.models[0].0, ModelName::new("res.partner"));
    }

    #[test]
    fn a_dtd_is_refused_so_no_entity_is_expanded() {
        let file = parse(&[
            "<?xml version='1.0'?>",
            "<!DOCTYPE odoo [",
            "  <!ENTITY a 'aaaaaaaaaa'>",
            "  <!ENTITY b '&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;'>",
            "  <!ENTITY c '&b;&b;&b;&b;&b;&b;&b;&b;&b;&b;'>",
            "]>",
            "<odoo><record id='x' model='res.partner'><field name='name'>&c;</field></record></odoo>",
        ]);
        assert!(file.data.records.is_empty());
        assert_eq!(file.diagnostics.len(), 1);
        assert_eq!(file.diagnostics[0].kind, DiagnosticKind::InvalidXml);
    }

    #[test]
    fn broken_and_too_deep_files_are_reported() {
        let broken = parse(&["<odoo>", "  <record id='x' model='res.partner'>", "</odoo>"]);
        assert_eq!(broken.diagnostics[0].kind, DiagnosticKind::InvalidXml);

        let deep = format!(
            "<odoo><record id='v' model='ir.ui.view'><field name='arch'>{}{}</field></record></odoo>",
            "<div>".repeat(MAX_DEPTH + 10),
            "</div>".repeat(MAX_DEPTH + 10)
        );
        let file = parse_xml(Path::new("views/deep.xml"), &deep);
        assert_eq!(file.diagnostics.len(), 1);
        assert_eq!(file.diagnostics[0].kind, DiagnosticKind::XmlTooDeep);
        assert_eq!(file.data.records.len(), 1);
    }

    #[test]
    fn other_root_elements_are_not_odoo_data() {
        let file = parse(&["<templates><t t-name='x'><field name='y'/></t></templates>"]);
        assert_eq!(file, XmlFile::default());
    }

    #[test]
    fn update_xmlids_names_records_made_elsewhere() {
        let file = parse(&[
            "<odoo>",
            "  <function model='ir.model.data' name='_update_xmlids'>",
            "    <value model='base' eval=\"[{",
            "        'xml_id': 'stock.stock_location_stock',",
            "        'record': obj().env.ref('stock.warehouse0').lot_stock_id,",
            "    }, {'record': obj(), 'xml_id': 'stock.picking_type_in'}, {'xml_id': name}]\"/>",
            "  </function>",
            "  <function model='ir.model.data' name='_update_xmlids'>",
            "    <value eval='not python ['/>",
            "  </function>",
            "  <function model='res.partner' name='_update_xmlids'>",
            "    <value eval=\"[{'xml_id': 'base.not_this'}]\"/>",
            "  </function>",
            "</odoo>",
        ]);
        let ids: Vec<(&str, u32)> = file
            .data
            .xmlids
            .iter()
            .map(|(xmlid, at)| (xmlid.as_str(), at.line.get()))
            .collect();
        assert_eq!(
            ids,
            [
                ("stock.stock_location_stock", 3),
                ("stock.picking_type_in", 3)
            ]
        );
    }
}
