//! Core data types for Pontix.
//!
//! This crate will hold the shared vocabulary of the project: symbols extracted
//! from Odoo code (models, fields, methods, views, XML ids), the changes between
//! two Odoo versions, and the findings reported to users (PTX codes).
#![forbid(unsafe_code)]

/// Version of the Pontix core crate.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::VERSION;

    #[test]
    fn version_is_set() {
        assert!(!VERSION.is_empty());
    }
}
