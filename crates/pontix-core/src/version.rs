//! Odoo versions and the rules that differ between them.
//!
//! Every rule that depends on the Odoo version lives here, matched
//! exhaustively, so supporting a new version means the compiler points at
//! each rule to decide. Each rule cites the core source it follows.

/// Odoo versions whose rules Pontix knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum OdooVersion {
    /// Odoo 18.0.
    V18,
    /// Odoo 19.0.
    V19,
}

impl OdooVersion {
    /// The series string, as in `odoo/release.py`: `18.0`, `19.0`.
    pub fn series(self) -> &'static str {
        match self {
            Self::V18 => "18.0",
            Self::V19 => "19.0",
        }
    }

    /// Manifest file names this version looks for in a module directory, in
    /// the order it tries them (`MANIFEST_NAMES`). Odoo 19 dropped
    /// `__openerp__.py`.
    pub fn manifest_file_names(self) -> &'static [&'static str] {
        match self {
            Self::V18 => &["__manifest__.py", "__openerp__.py"],
            Self::V19 => &["__manifest__.py"],
        }
    }

    /// Whether an empty `depends` means `['base']` (every module but `base`
    /// itself). Odoo 19 added this rule to `_load_manifest`.
    pub fn empty_depends_means_base(self) -> bool {
        match self {
            Self::V18 => false,
            Self::V19 => true,
        }
    }

    /// Whether a `test_` module loads right after its deepest dependency
    /// instead of in name order (`ModuleNode.order_name`, Odoo 19).
    pub fn test_modules_follow_dependency(self) -> bool {
        match self {
            Self::V18 => false,
            Self::V19 => true,
        }
    }

    /// Odoo's `adapt_version`: the canonical `<series>.x.y[.z]` form of a
    /// manifest version, or `None` where Odoo raises `ValueError`.
    ///
    /// Odoo 18 prefixes anything not already in the series and then requires
    /// `x.y` or `x.y.z` after the prefix, so another series is invalid. Odoo 19
    /// accepts two to five integer parts and prefixes only short versions, so
    /// another series stays as written and fails [`Self::is_own_series`].
    pub fn adapt_module_version(self, written: &str) -> Option<String> {
        let series = self.series();
        match self {
            Self::V18 => {
                let (adapted, rest) = match written.strip_prefix(&format!("{series}.")) {
                    Some(rest) => (written.to_string(), rest),
                    None => (format!("{series}.{written}"), written),
                };
                let parts: Vec<&str> = rest.split('.').collect();
                let well_formed = (2..=3).contains(&parts.len())
                    && parts
                        .iter()
                        .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
                well_formed.then_some(adapted)
            }
            Self::V19 => {
                let parts: Vec<&str> = written.split('.').collect();
                if !(2..=5).contains(&parts.len()) || !parts.iter().all(|part| is_python_int(part))
                {
                    return None;
                }
                Some(if parts.len() <= 3 && !written.starts_with(series) {
                    format!("{series}.{written}")
                } else {
                    written.to_string()
                })
            }
        }
    }

    /// Whether an adapted module version belongs to this series
    /// (`check_version`).
    pub fn is_own_series(self, adapted: &str) -> bool {
        adapted
            .strip_prefix(self.series())
            .is_some_and(|rest| rest.starts_with('.'))
    }
}

/// Whether Python's `int()` accepts `text`: optional surrounding whitespace,
/// an optional sign, and digits that may be grouped with single underscores.
fn is_python_int(text: &str) -> bool {
    let trimmed = text.trim();
    let digits = trimmed.strip_prefix(['+', '-']).unwrap_or(trimmed);
    !digits.is_empty()
        && !digits.starts_with('_')
        && !digits.ends_with('_')
        && !digits.contains("__")
        && digits.chars().all(|c| c.is_ascii_digit() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_and_own_series_versions_are_adapted() {
        for (version, written, adapted) in [
            (OdooVersion::V19, "19.0.1.0.0", "19.0.1.0.0"),
            (OdooVersion::V19, "1.0", "19.0.1.0"),
            (OdooVersion::V19, "1.2.3", "19.0.1.2.3"),
            (OdooVersion::V18, "18.0.1.0.0", "18.0.1.0.0"),
            (OdooVersion::V18, "1.0", "18.0.1.0"),
        ] {
            assert_eq!(
                version.adapt_module_version(written).as_deref(),
                Some(adapted),
                "{version:?} {written}"
            );
            assert!(version.is_own_series(adapted));
        }
    }

    #[test]
    fn another_series_is_kept_by_19_and_rejected_by_18() {
        let kept = OdooVersion::V19.adapt_module_version("18.0.1.0.0");
        assert_eq!(kept.as_deref(), Some("18.0.1.0.0"));
        assert!(!OdooVersion::V19.is_own_series("18.0.1.0.0"));
        assert_eq!(OdooVersion::V18.adapt_module_version("17.0.1.0.0"), None);
    }

    #[test]
    fn malformed_versions_are_rejected() {
        for (version, written) in [
            (OdooVersion::V19, "1"),
            (OdooVersion::V19, "1.0.0.0.0.0"),
            (OdooVersion::V19, "1.x"),
            (OdooVersion::V18, "1.0.0.0"),
            (OdooVersion::V18, "v1.0"),
        ] {
            assert_eq!(
                version.adapt_module_version(written),
                None,
                "{version:?} {written}"
            );
        }
    }

    #[test]
    fn python_int_accepts_what_python_accepts() {
        for text in ["0", "10", " 1 ", "+1", "-1", "1_000"] {
            assert!(is_python_int(text), "{text}");
        }
        for text in ["", "x", "1.0", "_1", "1_", "1__0", "+"] {
            assert!(!is_python_int(text), "{text}");
        }
    }
}
