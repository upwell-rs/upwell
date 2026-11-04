use std::fs;
use std::path::{Component, Path, PathBuf};

use tempfile::{Builder, TempDir};

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
    ///
    /// Callers must serialize runs that share `parent` and `prefix`.
    ///
    /// # Panics
    ///
    /// Panics when `prefix` is empty or contains a path separator, when
    /// `parent` cannot be created or read, or when a stale directory cannot be
    /// removed.
    pub fn new_in(parent: &Path, prefix: &str) -> Self {
        assert!(
            !prefix.is_empty() && !prefix.contains(['/', '\\']),
            "temporary fixture prefix must be non-empty and contain no path separators"
        );

        fs::create_dir_all(parent).expect("create temporary fixture parent directory");
        remove_stale(parent, prefix);

        let root = Builder::new()
            .prefix(prefix)
            .tempdir_in(parent)
            .expect("create temporary test fixture");

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
        let stale = entry.file_name().to_string_lossy().starts_with(prefix)
            && entry.file_type().is_ok_and(|kind| kind.is_dir());

        if stale {
            fs::remove_dir_all(entry.path()).expect("remove stale temporary fixture");
        }
    }
}
