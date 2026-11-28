//! What Python code does with records: the fields it reads and writes and
//! the models it opens, wherever the model is certain.
//!
//! Python is not typed, so a reference is only kept when its model follows
//! from the code itself: `self` in a model's method, `env["model"]` with a
//! literal name, or `env.ref("module.xmlid")`, and fields reached from them.
//! Everything else is left out, never guessed.

use crate::symbol::{Location, ModelName};

/// Where a reference starts.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CodeRoot {
    /// `self` in a method of the model class the reference is in.
    Own,
    /// `env["model"]`, with the model name written as a literal.
    Model(ModelName),
    /// `env.ref("module.xmlid")`: the record with that external id.
    Xmlid(String),
}

/// How the code uses the last field of a reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CodeUse {
    /// Reads or assigns an attribute, as in `order.partner_id.name`.
    Attribute,
    /// A key of the values passed to `create`, `write`, `new`, `update`,
    /// `copy` or `Command.create`.
    Values,
    /// The field path of a domain condition, as in
    /// `search([("partner_id.name", "=", x)])`.
    Domain,
    /// A field path passed as a string, as in `mapped("line_ids.price")`.
    FieldString,
}

/// A chain of fields Python code goes through, from a root whose model is
/// certain.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CodeRef {
    /// Where the chain starts.
    pub root: CodeRoot,
    /// The field names, in order. Empty when the code only opens the model,
    /// as in `env["res.partner"].search([])`.
    pub path: Vec<String>,
    /// How the last field is used.
    pub how: CodeUse,
    /// Where it is written.
    pub location: Location,
}
