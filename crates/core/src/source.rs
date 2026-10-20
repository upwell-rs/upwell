//! Static source positions recorded by descriptors, so diagnostics and tooling can point
//! back at the declaration that produced them.

use std::panic::Location;

/// Captures the call site's source location for descriptor diagnostics.
#[macro_export]
macro_rules! descriptor_source {
    () => {
        $crate::DescriptorSource::new(file!(), line!(), column!())
    };
}

/// Captures the call site as the [`ItemSource`] of the named item in the current module.
#[macro_export]
macro_rules! item_source {
    ($item:expr) => {
        $crate::ItemSource::new($crate::descriptor_source!(), ::core::module_path!(), $item)
    };
}

/// Static source location for descriptor diagnostics.
///
/// `file` is the path the compiler was given for the declaring source file, which for Cargo
/// workspace members is relative to the workspace root.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DescriptorSource {
    pub file: &'static str,
    pub line: u32,
    pub column: u32,
}

impl Default for DescriptorSource {
    fn default() -> Self {
        Self::UNKNOWN
    }
}

impl DescriptorSource {
    pub const UNKNOWN: Self = Self {
        file: "<unknown>",
        line: 0,
        column: 0,
    };

    pub const fn new(file: &'static str, line: u32, column: u32) -> Self {
        Self { file, line, column }
    }

    /// The location of the caller, through any chain of `#[track_caller]` functions.
    #[track_caller]
    pub const fn caller() -> Self {
        let location = Location::caller();

        Self::new(location.file(), location.line(), location.column())
    }
}

/// The declaration of a named Rust item: where it is written and the path that names it.
///
/// Macros record this at the user's own identifier, so `location` points at the item's name
/// and [`module_path`](Self::module_path) + [`item`](Self::item) form the canonical Rust path an
/// IDE can resolve by symbol (`my_app::http::Users`, or `my_app::http::Users::list` for a
/// method).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ItemSource {
    /// The position of the item's identifier.
    pub location: DescriptorSource,
    /// The declaring module, as `module_path!()` reports it (`crate_name::module`).
    pub module_path: &'static str,
    /// The item's path within its module: `Type`, or `Type::method` for an associated item.
    pub item: &'static str,
}

impl ItemSource {
    pub const fn new(
        location: DescriptorSource,
        module_path: &'static str,
        item: &'static str,
    ) -> Self {
        Self {
            location,
            module_path,
            item,
        }
    }

    /// The canonical Rust path of the item, `module_path::item`.
    pub fn rust_path(&self) -> String {
        format!("{}::{}", self.module_path, self.item)
    }
}

#[cfg(test)]
mod tests;
