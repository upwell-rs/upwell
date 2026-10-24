#![allow(dead_code)]

use std::fs;
use std::path::{Component, Path, PathBuf};

/// A temporary directory whose files tests write by relative path.
pub(crate) struct TempFixture {
    root: tempfile::TempDir,
}

impl TempFixture {
    pub(crate) fn new(prefix: &str) -> Self {
        let root = tempfile::Builder::new()
            .prefix(prefix)
            .tempdir()
            .expect("create temporary test fixture");

        Self { root }
    }

    pub(crate) fn path(&self) -> &Path {
        self.root.path()
    }

    pub(crate) fn child(&self, path: impl AsRef<Path>) -> PathBuf {
        let path = path.as_ref();

        assert!(
            path.components()
                .all(|component| matches!(component, Component::Normal(_) | Component::CurDir)),
            "temporary fixture children must be relative and cannot contain parent components"
        );

        self.path().join(path)
    }

    pub(crate) fn write(&self, path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> PathBuf {
        let path = self.child(path);

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create temporary fixture parent");
        }

        fs::write(&path, contents).expect("write temporary fixture file");

        path
    }
}
