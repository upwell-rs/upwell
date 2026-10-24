//! Conversion of descriptor declaration sites into tooling source locations.

use upwell_core::{DescriptorSource, ItemSource, TypeDescriptor};
use upwell_tooling_schema::{Provenance, SourceLocation};

/// The tooling location of a recorded declaration, or `None` for an unknown position.
pub(crate) fn item_location(source: &ItemSource) -> Option<SourceLocation> {
    let location = source.location;

    if location.line == 0 {
        return None;
    }

    Some(SourceLocation {
        file: location.file.to_string(),
        line: Some(location.line),
        column: (location.column > 0).then_some(location.column),
        rust_path: Some(source.rust_path()),
    })
}

/// The tooling location of a descriptor node, or `None` for an unknown position.
pub(crate) fn descriptor_location(source: DescriptorSource) -> Option<SourceLocation> {
    if source.line == 0 || source.file.is_empty() {
        return None;
    }

    Some(SourceLocation {
        file: source.file.to_string(),
        line: Some(source.line),
        column: (source.column > 0).then_some(source.column),
        rust_path: None,
    })
}

/// The declaration location of a described type, when its macro recorded one.
pub(crate) fn type_location(ty: &TypeDescriptor) -> Option<SourceLocation> {
    ty.source.as_ref().and_then(item_location)
}

/// Adds `source` to `provenance`, creating an otherwise empty provenance when absent.
pub(crate) fn with_source(
    provenance: Option<Provenance>,
    source: Option<SourceLocation>,
) -> Option<Provenance> {
    match (provenance, source) {
        (provenance, None) => provenance,
        (provenance, Some(source)) => Some(Provenance {
            source: Some(source),
            ..provenance.unwrap_or_default()
        }),
    }
}
