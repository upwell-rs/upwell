use std::panic;

use upwell_test_utils::{FIXTURE_MARKER, TempFixture};

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
    parent.write(format!("upwell-fixture-stale/{FIXTURE_MARKER}"), b"");
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

    let joined = std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let fixture = TempFixture::new_in(parent.path(), "upwell-fixture-");

                *observed.lock().expect("record fixture root") = Some(fixture.path().to_path_buf());

                panic!("simulated test failure");
            })
            .join()
    });
    let root = observed
        .lock()
        .expect("read fixture root")
        .take()
        .expect("fixture was created");

    assert!(joined.is_err());
    assert!(!root.exists());
}

#[test]
fn new_in_rejects_unsafe_prefixes_without_sweeping_siblings() {
    let parent = TempFixture::new("upwell-temp-parent-");
    let sibling = parent.write("other-fixture/keep.txt", b"keep");

    for prefix in ["", ".", "..", "a/b", "..\\x"] {
        assert!(panic::catch_unwind(|| TempFixture::new_in(parent.path(), prefix)).is_err());
    }

    assert!(sibling.exists());
}

#[cfg(windows)]
#[test]
fn new_in_rejects_a_windows_drive_prefix() {
    let parent = TempFixture::new("upwell-temp-parent-");

    assert!(panic::catch_unwind(|| TempFixture::new_in(parent.path(), "C:")).is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn stale_sweep_ignores_names_that_are_not_valid_utf8() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt as _;

    let parent = TempFixture::new("upwell-temp-parent-");
    let invalid = parent
        .path()
        .join(OsStr::from_bytes(b"\xffinvalid-fixture"));

    std::fs::create_dir(&invalid).expect("create invalid-name directory");
    std::fs::write(invalid.join(FIXTURE_MARKER), b"").expect("mark invalid-name directory");

    let _fixture = TempFixture::new_in(parent.path(), "\u{fffd}invalid-");

    assert!(invalid.exists());
}

#[test]
fn stale_sweep_keeps_prefixed_directories_without_the_fixture_marker() {
    let parent = TempFixture::new("upwell-temp-parent-");
    let unmarked = parent.write("upwell-fixture-results/report.txt", b"keep");

    let _fixture = TempFixture::new_in(parent.path(), "upwell-fixture-");

    assert!(unmarked.exists());
}
