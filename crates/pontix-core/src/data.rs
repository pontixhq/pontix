//! What Odoo data files declare, exactly as written: records with their
//! external ids, view architectures, and references to other records by
//! external id (`ref=`, `ref('...')`, `groups=`, `env.ref('...')`).

use crate::symbol::{FieldName, Location, ModelName};

/// What one XML data file listed in a manifest declares.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DataFile {
    /// `<record>`, `<template>` and `<menuitem>` elements, in file order.
    pub records: Vec<Record>,
    /// Models named by `<function model=...>` and `<delete model=...>`.
    pub models: Vec<(ModelName, Location)>,
    /// References to records by external id, in file order.
    pub refs: Vec<XmlRef>,
    /// External ids that `ir.model.data._update_xmlids` gives records made
    /// elsewhere, such as `stock.stock_location_stock`, as written.
    pub xmlids: Vec<(String, Location)>,
}

/// A record the file creates or updates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// The external id as written: `xmlid` for one of this module, or
    /// `module.xmlid` for one of another module. `None` when the element
    /// has no id.
    pub id: Option<String>,
    /// The model of the record: `ir.ui.view` for a `<template>`,
    /// `ir.ui.menu` for a `<menuitem>`.
    pub model: ModelName,
    /// The `<field name=...>` elements that set its values.
    pub fields: Vec<(FieldName, Location)>,
    /// The view it declares, for an `ir.ui.view` record or a `<template>`.
    pub view: Option<View>,
    /// Where the element is.
    pub location: Location,
}

/// A view: its model, the view it inherits, and the fields its architecture
/// shows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct View {
    /// `<field name="model">`. An inherited view often leaves it out and
    /// takes the model of the view it inherits.
    pub model: Option<ModelName>,
    /// The external id of the view it inherits, as written.
    pub inherit_id: Option<String>,
    /// The `<field>` elements of its architecture. A QWeb `<template>` has
    /// none: its fields are expressions, which Pontix does not read.
    pub fields: Vec<ArchField>,
}

/// A `<field name=...>` element in a view architecture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchField {
    /// The field.
    pub name: FieldName,
    /// The x2many fields whose embedded views hold it, outermost first:
    /// `product_id` inside `<field name="order_line"><list>` is a field of
    /// the comodel of `order_line`. Empty for a field of the view's model.
    pub within: Vec<FieldName>,
    /// Whether it only locates a place in the inherited view
    /// (`<field name="partner_id" position="after">`).
    pub locator: bool,
    /// Where the element is.
    pub location: Location,
}

/// How a data file or Python code refers to a record by external id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RefKind {
    /// `<field ref="...">`.
    Ref,
    /// `ref('...')` in an `eval`.
    Eval,
    /// A `groups` attribute.
    Groups,
    /// A `<menuitem>`'s `action`, or `%(...)d` in an attribute, as a
    /// button's `name`.
    Action,
    /// A `<menuitem>`'s `parent`.
    Parent,
    /// The view an `ir.ui.view` or `<template>` inherits.
    InheritId,
    /// `env.ref('...')` in Python.
    Python,
    /// `env.ref('...')` in the module's tests.
    Test,
}

/// A reference to a record by external id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlRef {
    /// The external id as written, with or without its module.
    pub xmlid: String,
    /// How it is referred to.
    pub kind: RefKind,
    /// Where.
    pub location: Location,
}
