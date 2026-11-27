//! Finds Odoo modules in addons paths and reads what Odoo would load.
//!
//! Only files Odoo actually imports are read: the chain of imports starting
//! at each module's `__init__.py`. A Python file that nothing imports never
//! runs in Odoo, so its models are not indexed either.
//!
//! Inputs are untrusted. Files are parsed, never executed; paths that leave
//! the addons path through a symbolic link are not followed; files over
//! [`MAX_FILE_SIZE`] are not read.
#![forbid(unsafe_code)]

mod files;
mod load;
mod scan;

pub use files::MAX_FILE_SIZE;
pub use scan::{Addons, Module, scan};
