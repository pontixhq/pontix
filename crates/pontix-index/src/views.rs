//! Views as Odoo combines them: the architecture of a view with the views
//! extending it applied, in the order Odoo applies them, as far as the
//! modules in a scope go.
//!
//! What Pontix cannot apply with certainty (an XPath it does not read, a
//! spec that finds nothing, a view inheriting itself) makes the combined
//! architecture unknown rather than wrong: callers stay silent on it.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use pontix_core::{Element, Location, ModuleName, View};
use pontix_parsers::XPath;

use crate::install::Installation;
use crate::registry::qualified;

/// Odoo's default view priority.
const DEFAULT_PRIORITY: i64 = 16;

/// Longest chain of inherited views followed.
const MAX_CHAIN: usize = 32;

/// A root view and the extensions applied to it, by index.
type Combination = (usize, Vec<usize>);

/// Every view of an installation, by external id.
#[derive(Debug)]
pub struct Views<'a> {
    entries: Vec<Entry<'a>>,
    by_id: HashMap<String, usize>,
    /// The views inheriting each view, by its id.
    inheriting: HashMap<String, Vec<usize>>,
    /// Architectures already combined, by root view and the extensions
    /// applied: modules with the same dependencies ask for the same ones.
    combined: RefCell<HashMap<Combination, Option<Element>>>,
}

/// One view of an installation.
#[derive(Debug, Clone)]
pub struct Entry<'a> {
    /// Its external id, as `module.xmlid`.
    pub id: String,
    /// The module whose data declared it last: a later declaration of the
    /// same id replaces the view.
    pub module: &'a ModuleName,
    /// The view.
    pub view: &'a View,
    /// Where it is declared.
    pub location: &'a Location,
    /// The external id of the view it inherits, as `module.xmlid`.
    pub parent: Option<String>,
    /// Odoo applies extensions by priority, then by creation: load order.
    order: (i64, usize),
}

impl Entry<'_> {
    /// Whether it extends the view it inherits instead of making its own.
    fn extends(&self) -> bool {
        self.parent.is_some() && !self.view.primary
    }
}

/// What locating an inheriting view's spec gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Located {
    /// The element, as the indices of children from the root.
    Found(Vec<usize>),
    /// Nothing matches.
    Missing,
    /// Pontix cannot tell, such as for an XPath it does not read.
    Unknown,
}

impl<'a> Views<'a> {
    /// The views of the installed modules, in load order.
    pub fn new(installation: &Installation<'a>) -> Self {
        let mut views = Self {
            entries: Vec::new(),
            by_id: HashMap::new(),
            inheriting: HashMap::new(),
            combined: RefCell::default(),
        };
        for module in &installation.modules {
            for (_, data) in &module.data {
                for record in &data.records {
                    let (Some(id), Some(view)) = (&record.id, &record.view) else {
                        continue;
                    };
                    let id = qualified(&module.name, id);
                    let parent = view
                        .inherit_id
                        .as_deref()
                        .map(|parent| qualified(&module.name, parent));
                    let position = views.entries.len();
                    let entry = Entry {
                        id: id.clone(),
                        module: &module.name,
                        view,
                        location: &record.location,
                        parent,
                        order: (view.priority.unwrap_or(DEFAULT_PRIORITY), position),
                    };
                    match views.by_id.get(&id) {
                        // An update keeps the record, and so its place.
                        Some(&index) => {
                            let order = views.entries[index].order.1;
                            views.entries[index] = Entry {
                                order: (entry.order.0, order),
                                ..entry
                            };
                        }
                        None => {
                            views.by_id.insert(id, position);
                            views.entries.push(entry);
                        }
                    }
                }
            }
        }
        for (index, entry) in views.entries.iter().enumerate() {
            if let Some(parent) = &entry.parent {
                views
                    .inheriting
                    .entry(parent.clone())
                    .or_default()
                    .push(index);
            }
        }
        views
    }

    /// A view by external id.
    pub fn get(&self, id: &str) -> Option<&Entry<'a>> {
        self.by_id.get(id).map(|&index| &self.entries[index])
    }

    /// The architecture the specs of view `id` are applied to: the view it
    /// inherits, combined with every extension Odoo applies before this
    /// one, of the modules `in_scope` keeps. `None` when that cannot be
    /// told, or when the inherited view is not there.
    pub fn before(&self, id: &str, in_scope: &dyn Fn(&ModuleName) -> bool) -> Option<Element> {
        let entry = self.get(id)?;
        let parent = entry.parent.as_deref()?;
        let root = self.root(parent)?;
        // Every view up to the root must be there.
        let mut up = parent;
        while up != root {
            let view = self.get(up)?;
            if !in_scope(view.module) {
                return None;
            }
            up = view.parent.as_deref()?;
        }
        let limit = entry.extends().then_some(entry.order);
        self.combined(root, limit, Some(id), in_scope, 0)
    }

    /// The view whose architecture an extension chain starts from: the
    /// first one up the chain that is not an extension.
    fn root(&self, id: &str) -> Option<&str> {
        let mut id = id;
        for _ in 0..MAX_CHAIN {
            let entry = self.get(id)?;
            if !entry.extends() {
                return Some(&entry.id);
            }
            id = entry.parent.as_deref()?;
        }
        None
    }

    /// The architecture of view `root` with its extensions applied, those
    /// before `limit` only when it is given, `skip` left out.
    fn combined(
        &self,
        root: &str,
        limit: Option<(i64, usize)>,
        skip: Option<&str>,
        in_scope: &dyn Fn(&ModuleName) -> bool,
        depth: usize,
    ) -> Option<Element> {
        if depth > MAX_CHAIN {
            return None;
        }
        let index = *self.by_id.get(root)?;
        let entry = &self.entries[index];
        if !in_scope(entry.module) {
            return None;
        }
        let extensions = self.extensions(root, limit, skip, in_scope);
        let key = (index, extensions.clone());
        if let Some(known) = self.combined.borrow().get(&key) {
            return known.clone();
        }
        let arch = self.apply_all(entry, &extensions, in_scope, depth);
        self.combined.borrow_mut().insert(key, arch.clone());
        arch
    }

    fn apply_all(
        &self,
        entry: &Entry<'a>,
        extensions: &[usize],
        in_scope: &dyn Fn(&ModuleName) -> bool,
        depth: usize,
    ) -> Option<Element> {
        let mut arch = match &entry.parent {
            // A primary view starts from the view it inherits.
            Some(parent) => {
                let mut arch =
                    self.combined(self.root(parent)?, None, None, in_scope, depth + 1)?;
                apply(&mut arch, entry.view.arch.as_ref()?).ok()?;
                arch
            }
            None => entry.view.arch.clone()?,
        };
        for &extension in extensions {
            apply(&mut arch, self.entries[extension].view.arch.as_ref()?).ok()?;
        }
        Some(arch)
    }

    /// The extensions of a view, its own and those of its extensions, in
    /// the order Odoo applies them: by priority then load order, each after
    /// the view it extends.
    fn extensions(
        &self,
        root: &str,
        limit: Option<(i64, usize)>,
        skip: Option<&str>,
        in_scope: &dyn Fn(&ModuleName) -> bool,
    ) -> Vec<usize> {
        let mut found: Vec<usize> = Vec::new();
        let mut pending = vec![root.to_string()];
        let mut seen: HashSet<String> = HashSet::from([root.to_string()]);
        while let Some(id) = pending.pop() {
            for &index in self.inheriting.get(&id).into_iter().flatten() {
                let extension = &self.entries[index];
                if !extension.extends()
                    || Some(extension.id.as_str()) == skip
                    || !in_scope(extension.module)
                    || limit.is_some_and(|limit| extension.order >= limit)
                    || !seen.insert(extension.id.clone())
                {
                    continue;
                }
                found.push(index);
                pending.push(extension.id.clone());
            }
        }
        found.sort_by_key(|&index| self.entries[index].order);
        // Each after the view it extends.
        let mut ordered: Vec<usize> = Vec::with_capacity(found.len());
        let mut applied: HashSet<&str> = HashSet::from([root]);
        while !found.is_empty() {
            let next = found
                .iter()
                .position(|&index| {
                    self.entries[index]
                        .parent
                        .as_deref()
                        .is_some_and(|parent| applied.contains(parent))
                })
                .unwrap_or(0);
            let index = found.remove(next);
            applied.insert(&self.entries[index].id);
            ordered.push(index);
        }
        ordered
    }
}

/// Where an inheriting view's spec applies, as Odoo's `locate_node` finds
/// it: the first match of an `<xpath>`, the first `<field>` of that name,
/// or the first element of that tag whose attributes all match.
pub fn locate(arch: &Element, spec: &Element) -> Located {
    if spec.tag == "xpath" {
        let Some(expr) = spec.attribute("expr") else {
            return Located::Unknown;
        };
        return match XPath::parse(expr).and_then(|xpath| xpath.select(arch)) {
            Ok(found) => found
                .into_iter()
                .next()
                .map_or(Located::Missing, Located::Found),
            Err(_) => Located::Unknown,
        };
    }
    let matches = |element: &Element| {
        element.tag == spec.tag
            && if spec.tag == "field" {
                element.attribute("name") == spec.attribute("name")
            } else {
                spec.attributes
                    .iter()
                    .filter(|(name, _)| name != "position" && name != "version")
                    .all(|(name, value)| element.attribute(name) == Some(value.as_str()))
            }
    };
    let mut pending = vec![(arch, Vec::new())];
    while let Some((element, path)) = pending.pop() {
        if matches(element) {
            return Located::Found(path);
        }
        for (index, child) in element.children.iter().enumerate().rev() {
            let mut child_path = path.clone();
            child_path.push(index);
            pending.push((child, child_path));
        }
    }
    Located::Missing
}

/// The specs of an inheriting view: the children of a `<data>` root, or
/// the root itself.
pub fn specs(arch: &Element) -> Vec<&Element> {
    if arch.tag != "data" {
        return vec![arch];
    }
    let mut found = Vec::new();
    for child in &arch.children {
        if child.tag == "data" {
            found.extend(specs(child));
        } else {
            found.push(child);
        }
    }
    found
}

/// The architecture after an inheriting view, as Odoo's
/// `apply_inheritance_specs` makes it. A spec that finds nothing, or that
/// Pontix cannot apply, fails the whole view.
fn apply(arch: &mut Element, view: &Element) -> Result<(), ()> {
    for spec in specs(view) {
        spec_applied(arch, spec)?;
    }
    Ok(())
}

/// Applies one spec of an inheriting view: `false` when it finds nothing or
/// Pontix cannot apply it.
pub fn apply_spec(arch: &mut Element, spec: &Element) -> bool {
    spec_applied(arch, spec).is_ok()
}

fn spec_applied(arch: &mut Element, spec: &Element) -> Result<(), ()> {
    {
        let Located::Found(path) = locate(arch, spec) else {
            return Err(());
        };
        let position = spec.attribute("position").unwrap_or("inside");
        match position {
            "attributes" => {
                let node = at_mut(arch, &path).ok_or(())?;
                for attribute in spec
                    .children
                    .iter()
                    .filter(|child| child.tag == "attribute")
                {
                    set_attribute(node, attribute)?;
                }
            }
            "inside" => {
                let (content, path) = content(arch, spec, path)?;
                at_mut(arch, &path).ok_or(())?.children.extend(content);
            }
            "after" | "before" => {
                let (content, path) = content(arch, spec, path)?;
                let (&index, parent) = path.split_last().ok_or(())?;
                let at = if position == "after" {
                    index + 1
                } else {
                    index
                };
                let parent = at_mut(arch, parent).ok_or(())?;
                parent.children.splice(at..at, content);
            }
            "replace" => {
                let original = arch.at(&path).ok_or(())?.clone();
                let (mut content, path) = content(arch, spec, path)?;
                for element in &mut content {
                    restore_original(element, &original);
                }
                if spec.attribute("mode") == Some("inner") {
                    at_mut(arch, &path).ok_or(())?.children = content;
                } else if let Some((&index, parent)) = path.split_last() {
                    let parent = at_mut(arch, parent).ok_or(())?;
                    parent.children.splice(index..=index, content);
                } else {
                    // The root replaced: by the spec's first element.
                    *arch = content.into_iter().next().ok_or(())?;
                }
            }
            _ => return Err(()),
        }
    }
    Ok(())
}

/// The elements a spec brings, with those it moves (`position="move"`)
/// taken out of the architecture, and the path of the spec's target once
/// they are.
fn content(
    arch: &mut Element,
    spec: &Element,
    mut target: Vec<usize>,
) -> Result<(Vec<Element>, Vec<usize>), ()> {
    let mut content = Vec::with_capacity(spec.children.len());
    for child in &spec.children {
        if child.attribute("position") != Some("move") {
            content.push(child.clone());
            continue;
        }
        let Located::Found(moved) = locate(arch, child) else {
            return Err(());
        };
        // Moving the target, or an element holding it, cannot be done.
        if target.starts_with(&moved) {
            return Err(());
        }
        let (&index, parent) = moved.split_last().ok_or(())?;
        let element = at_mut(arch, parent).ok_or(())?.children.remove(index);
        // A sibling before the target, or before one of its ancestors,
        // shifts it.
        if target.len() > parent.len() && target.starts_with(parent) && target[parent.len()] > index
        {
            target[parent.len()] -= 1;
        }
        content.push(element);
    }
    Ok((content, target))
}

/// `$0` in a `replace` spec: the element replaced.
fn restore_original(element: &mut Element, original: &Element) {
    if element.text.as_deref() == Some("$0") {
        element.text = None;
        element.children.push(original.clone());
        return;
    }
    for child in &mut element.children {
        restore_original(child, original);
    }
}

/// `<attribute name="...">`: a value, or `add` / `remove` with a separator.
fn set_attribute(node: &mut Element, attribute: &Element) -> Result<(), ()> {
    let name = attribute.attribute("name").ok_or(())?;
    let current = node.attribute(name).map(str::to_string);
    let value = match (attribute.attribute("add"), attribute.attribute("remove")) {
        (None, None) => attribute.text.clone(),
        (add, remove) => {
            let separator = attribute.attribute("separator").unwrap_or(",");
            let mut values: Vec<String> = current
                .as_deref()
                .unwrap_or_default()
                .split(separator)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .collect();
            if let Some(remove) = remove {
                let removed: Vec<&str> = remove.split(separator).map(str::trim).collect();
                values.retain(|value| !removed.contains(&value.as_str()));
            }
            if let Some(add) = add {
                for value in add.split(separator).map(str::trim) {
                    if !value.is_empty() && !values.iter().any(|own| own == value) {
                        values.push(value.to_string());
                    }
                }
            }
            let joined = values.join(separator);
            (!joined.is_empty()).then_some(joined)
        }
    };
    node.attributes.retain(|(own, _)| own != name);
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        node.attributes.push((name.to_string(), value));
    }
    Ok(())
}

fn at_mut<'e>(element: &'e mut Element, path: &[usize]) -> Option<&'e mut Element> {
    path.iter()
        .try_fold(element, |element, &index| element.children.get_mut(index))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pontix_core::{Manifest, OdooVersion};
    use pontix_parsers::parse_xml;

    use super::*;
    use crate::scan::Module;

    fn module(name: &str, xml: &str) -> Module {
        let file = PathBuf::from(format!("{name}/views.xml"));
        let parsed = parse_xml(&file, &format!("<odoo>{xml}</odoo>"));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        Module {
            name: ModuleName::new(name),
            path: PathBuf::from(name),
            manifest_path: PathBuf::from(format!("{name}/__manifest__.py")),
            manifest: Manifest::default(),
            python_files: Vec::new(),
            models: Vec::new(),
            xmlid_refs: Vec::new(),
            code_refs: Vec::new(),
            test_refs: Vec::new(),
            data: vec![(file, parsed.data)],
        }
    }

    fn view(id: &str, inherit: Option<&str>, extra: &str, arch: &str) -> String {
        let inherit = inherit
            .map(|parent| format!("<field name='inherit_id' ref='{parent}'/>"))
            .unwrap_or_default();
        format!(
            "<record id='{id}' model='ir.ui.view'>{inherit}{extra}<field name='arch' type='xml'>{arch}</field></record>"
        )
    }

    fn installation(modules: &[Module]) -> Installation<'_> {
        Installation {
            version: OdooVersion::V19,
            modules: modules.iter().collect(),
            unknown: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    /// The names of the fields, in document order.
    fn fields(element: &Element) -> Vec<String> {
        let mut found = Vec::new();
        if element.tag == "field" {
            found.push(element.attribute("name").unwrap_or_default().to_string());
        }
        for child in &element.children {
            found.extend(fields(child));
        }
        found
    }

    fn sale() -> Module {
        module(
            "sale",
            &view(
                "view_order_form",
                None,
                "",
                "<form><sheet><group name='main'><field name='partner_id'/><field name='date_order'/></group></sheet></form>",
            ),
        )
    }

    #[test]
    fn extensions_apply_in_order_and_only_before_the_view_asked_about() {
        let modules = [
            sale(),
            module(
                "sale_extra",
                &[
                    view(
                        "add_ref",
                        Some("sale.view_order_form"),
                        "",
                        "<field name='partner_id' position='after'><field name='client_ref'/></field>",
                    ),
                    view(
                        "early",
                        Some("sale.view_order_form"),
                        "<field name='priority'>5</field>",
                        "<xpath expr=\"//group[@name='main']\" position='inside'><field name='note'/></xpath>",
                    ),
                ]
                .concat(),
            ),
            module(
                "mine",
                &view(
                    "mine",
                    Some("sale_extra.add_ref"),
                    "",
                    "<data><field name='client_ref' position='replace'><field name='ref'/></field><xpath expr='//sheet' position='before'><header/></xpath></data>",
                ),
            ),
        ];
        let installation = installation(&modules);
        let views = Views::new(&installation);
        let everywhere = |_: &ModuleName| true;
        // `early` comes first by priority; `mine` is not applied to itself.
        let before = views
            .before("mine.mine", &everywhere)
            .expect("an architecture");
        assert_eq!(
            fields(&before),
            ["partner_id", "client_ref", "date_order", "note"]
        );
        // Out of scope, an extension is not there.
        let without_extra = |module: &ModuleName| module.as_str() != "sale_extra";
        assert_eq!(views.before("mine.mine", &without_extra), None);
        let only_sale = |module: &ModuleName| module.as_str() == "sale";
        let before = views
            .before("sale_extra.early", &only_sale)
            .expect("the base view");
        assert_eq!(fields(&before), ["partner_id", "date_order"]);

        let mut arch = before.clone();
        apply(
            &mut arch,
            views.get("mine.mine").unwrap().view.arch.as_ref().unwrap(),
        )
        .expect_err("client_ref is not in the base view");
        let mut arch = views.before("mine.mine", &everywhere).unwrap();
        apply(
            &mut arch,
            views.get("mine.mine").unwrap().view.arch.as_ref().unwrap(),
        )
        .expect("applies");
        assert_eq!(fields(&arch), ["partner_id", "ref", "date_order", "note"]);
        assert_eq!(arch.children[0].tag, "header");
    }

    #[test]
    fn specs_replace_move_and_change_attributes_as_odoo_does() {
        let modules = [
            sale(),
            module(
                "mine",
                &[
                    view(
                        "wrap",
                        Some("sale.view_order_form"),
                        "",
                        "<field name='partner_id' position='replace'><div class='wrapper'>$0</div></field>",
                    ),
                    view(
                        "move",
                        Some("sale.view_order_form"),
                        "",
                        "<xpath expr='//sheet' position='before'><xpath expr=\"//field[@name='date_order']\" position='move'/></xpath>",
                    ),
                    view(
                        "attrs",
                        Some("sale.view_order_form"),
                        "",
                        "<group name='main' position='attributes'><attribute name='string'>Main</attribute><attribute name='class' add='o_wide o_tall' separator=' '/><attribute name='name'/></group>",
                    ),
                    view(
                        "primary",
                        Some("sale.view_order_form"),
                        "<field name='mode'>primary</field>",
                        "<xpath expr='//form' position='attributes'><attribute name='create'>0</attribute></xpath>",
                    ),
                ]
                .concat(),
            ),
        ];
        let installation = installation(&modules);
        let views = Views::new(&installation);
        let everywhere = |_: &ModuleName| true;
        let arch = views
            .combined("sale.view_order_form", None, None, &everywhere, 0)
            .expect("an architecture");
        // `date_order` moved before the sheet; `partner_id` kept, wrapped.
        assert_eq!(arch.children[0].attribute("name"), Some("date_order"));
        let group = arch.at(&[1, 0]).expect("the group");
        assert_eq!(group.attribute("string"), Some("Main"));
        assert_eq!(group.attribute("class"), Some("o_wide o_tall"));
        assert_eq!(group.attribute("name"), None);
        let wrapper = group.at(&[0]).expect("the wrapper");
        assert_eq!(wrapper.attribute("class"), Some("wrapper"));
        assert_eq!(wrapper.children[0].attribute("name"), Some("partner_id"));
        // A primary view is a view of its own, not an extension.
        assert_eq!(arch.attribute("create"), None);
        let primary = views
            .combined("mine.primary", None, None, &everywhere, 0)
            .expect("the primary view");
        assert_eq!(primary.attribute("create"), Some("0"));
        assert_eq!(fields(&primary), fields(&arch));
    }

    #[test]
    fn what_cannot_be_applied_leaves_the_architecture_unknown() {
        let modules = [
            sale(),
            module(
                "other",
                &view(
                    "text",
                    Some("sale.view_order_form"),
                    "",
                    "<xpath expr=\"//label[text()='Customer']\" position='after'><field name='x'/></xpath>",
                ),
            ),
            module(
                "mine",
                &view(
                    "mine",
                    Some("sale.view_order_form"),
                    "",
                    "<field name='partner_id' position='after'/>",
                ),
            ),
            module(
                "loop",
                &[
                    view(
                        "a",
                        Some("loop.b"),
                        "<field name='mode'>primary</field>",
                        "<form/>",
                    ),
                    view(
                        "b",
                        Some("loop.a"),
                        "<field name='mode'>primary</field>",
                        "<form/>",
                    ),
                ]
                .concat(),
            ),
        ];
        let installation = installation(&modules);
        let views = Views::new(&installation);
        let everywhere = |_: &ModuleName| true;
        assert_eq!(views.before("mine.mine", &everywhere), None);
        let without_other = |module: &ModuleName| module.as_str() != "other";
        assert!(views.before("mine.mine", &without_other).is_some());
        assert_eq!(views.combined("loop.a", None, None, &everywhere, 0), None);
        let arch = views.before("mine.mine", &without_other).unwrap();
        let spec = &views.get("other.text").unwrap().view.arch.as_ref().unwrap();
        assert_eq!(locate(&arch, spec), Located::Unknown);
        let spec = &views.get("mine.mine").unwrap().view.arch.as_ref().unwrap();
        assert_eq!(locate(&arch, spec), Located::Found(vec![0, 0, 0]));
    }
}
