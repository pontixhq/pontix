//! Parsers for Odoo source files.
//!
//! Python files (models, fields, methods) and XML files (views, templates,
//! records) are parsed here and turned into `pontix-core` symbols.
#![forbid(unsafe_code)]

#[cfg(test)]
mod tests {
    #[test]
    fn core_is_linked() {
        assert!(!pontix_core::VERSION.is_empty());
    }
}
