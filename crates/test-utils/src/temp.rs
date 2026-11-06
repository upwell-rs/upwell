use std::fs;
use std::path::{Component, Path, PathBuf};

use tempfile::{Builder, TempDir};

/// Name of the file `TempFixture::new_in` writes into each fixture root so a later sweep recognizes it.
pub const FIXTURE_MARKER: &str = ".upwell-temp-fixture";

/// Owns a uniquely named temporary filesystem fixture.
pub struct TempFixture {
    root: TempDir,
}

impl TempFixture {
    /// Creates an automatically removed fixture with a descriptive prefix.
    pub fn new(prefix: &str) -> Self {
        let root = Builder::new()
            .prefix(prefix)
            .tempdir()
            .expect("create temporary test fixture");

        Self { root }
    }

    /// Creates an automatically removed fixture below `parent`, first removing
    /// directories with the same prefix that an earlier killed run left behind.
    /// Only directories holding the `FIXTURE_MARKER` file are removed, so an
    /// unrelated sibling that merely shares the prefix is left alone.
    ///
    /// Callers must serialize runs that share `parent` and `prefix`.
    ///
    /// # Panics
    ///
    /// Panics when `prefix` is empty, is `.` or `..`, contains a path separator or starts with a
    /// Windows path prefix such as `C:`, when `parent` cannot be created or
    /// read, or when a stale directory cannot be removed.
    pub fn new_in(parent: &Path, prefix: &str) -> Self {
        assert!(
            is_plain_prefix(prefix),
            "temporary fixture prefix must be non-empty and contain no path separators or drive prefix"
        );

        fs::create_dir_all(parent).expect("create temporary fixture parent directory");
        remove_stale(parent, prefix);

        let root = Builder::new()
            .prefix(prefix)
            .tempdir_in(parent)
            .expect("create temporary test fixture");

        fs::write(root.path().join(FIXTURE_MARKER), b"").expect("write temporary fixture marker");

        Self { root }
    }

    /// Returns the fixture root.
    pub fn path(&self) -> &Path {
        self.root.path()
    }

    /// Returns a path below the fixture root.
    pub fn child(&self, path: impl AsRef<Path>) -> PathBuf {
        let path = path.as_ref();

        assert!(
            path.components()
                .all(|component| matches!(component, Component::Normal(_) | Component::CurDir)),
            "temporary fixture children must be relative and cannot contain parent components"
        );

        self.path().join(path)
    }

    /// Writes a file below the fixture root, creating parent directories.
    pub fn write(&self, path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> PathBuf {
        let path = self.child(path);

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create temporary fixture parent");
        }

        fs::write(&path, contents).expect("write temporary fixture file");

        path
    }
}

fn remove_stale(parent: &Path, prefix: &str) {
    let entries = fs::read_dir(parent).expect("read temporary fixture parent directory");

    for entry in entries.flatten() {
        let name = entry.file_name();
        let stale = name.as_encoded_bytes().starts_with(prefix.as_bytes())
            && entry.file_type().is_ok_and(|kind| kind.is_dir())
            && entry.path().join(FIXTURE_MARKER).is_file();

        if stale {
            fs::remove_dir_all(entry.path()).expect("remove stale temporary fixture");
        }
    }
}

fn is_plain_prefix(prefix: &str) -> bool {
    !prefix.is_empty()
        && !matches!(prefix, "." | "..")
        && !prefix.contains(['/', '\\'])
        && !Path::new(prefix)
            .components()
            .any(|component| matches!(component, Component::Prefix(_)))
}
