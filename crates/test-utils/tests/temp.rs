use std::panic;

use upwell_test_utils::TempFixture;

#[test]
fn writes_are_scoped_to_the_fixture() {
    let fixture = TempFixture::new("upwell-temp-fixture-");
    let path = fixture.write("nested/value.txt", b"value");

    assert_eq!(path, fixture.child("nested/value.txt"));
    assert_eq!(
        std::fs::read(path).expect("fixture file is readable"),
        b"value"
    );
}

#[test]
fn child_rejects_paths_that_escape_the_fixture() {
    let fixture = TempFixture::new("upwell-temp-fixture-");

    assert!(panic::catch_unwind(|| fixture.child("../outside")).is_err());
    assert!(panic::catch_unwind(|| fixture.child(std::env::temp_dir())).is_err());
}

#[test]
fn fixture_below_a_parent_is_removed_on_drop_and_stale_siblings_are_swept() {
    let parent = TempFixture::new("upwell-temp-parent-");
    let stale = parent.write("upwell-fixture-stale/leftover.txt", b"old");
    let unrelated = parent.write("unrelated/keep.txt", b"keep");
    let fixture = TempFixture::new_in(parent.path(), "upwell-fixture-");
    let root = fixture.path().to_path_buf();

    assert!(!stale.exists());
    assert!(unrelated.exists());
    assert!(root.starts_with(parent.path()));

    drop(fixture);

    assert!(!root.exists());
}

#[test]
fn fixture_below_a_parent_is_removed_when_the_test_panics() {
    let parent = TempFixture::new("upwell-temp-parent-");
    let observed = std::sync::Mutex::new(None);

    let result = panic::catch_unwind(|| {
        let fixture = TempFixture::new_in(parent.path(), "upwell-fixture-");

        *observed.lock().expect("record fixture root") = Some(fixture.path().to_path_buf());

        panic!("simulated test failure");
    });
    let root = observed
        .lock()
        .expect("read fixture root")
        .clone()
        .expect("fixture was created");

    assert!(result.is_err());
    assert!(!root.exists());
}
