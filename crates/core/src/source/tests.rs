use super::*;

#[test]
fn item_source_names_the_canonical_rust_path() {
    let line = line!();
    let source = crate::item_source!("Users::list");

    assert_eq!(source.module_path, "upwell_core::source::tests");
    assert_eq!(
        source.rust_path(),
        "upwell_core::source::tests::Users::list"
    );
    assert!(source.location.file.ends_with("source/tests.rs"));
    assert_eq!(source.location.line, line + 1);
}

#[test]
fn caller_reports_the_calling_line() {
    let line = line!();
    let source = DescriptorSource::caller();

    assert_eq!(source.line, line + 1);
}
