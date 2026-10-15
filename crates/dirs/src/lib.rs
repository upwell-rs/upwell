//! Unified application directories, injectable as `Dir<K>`.
//!
//! A [`DirectoriesManager`] (built from project metadata, typically in `main`)
//! resolves the platform's per-application directories — config, data, cache, state,
//! runtime, and temp — via the `directories` crate's XDG / Known-Folder logic. Each
//! is handed out as a typed [`Dir<K>`] keyed by a marker ([`Config`], [`Runtime`],
//! [`Tmp`], …), so a component injects exactly the directory it needs:
//!
//! ```ignore
//! #[component]
//! struct Store { data: Dir<Data> }
//! ```
//!
//! The manager's directory list is consumed by the config crate (to resolve `${@runtime}`
//! and friends) and seeded into the daemon, so directory resolution is defined once. This
//! crate is standalone — it depends only on `upwell-core` and `upwell-di`, and knows
//! nothing about config.

use std::marker::PhantomData;
use std::ops::Deref;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::sync::{Arc, Mutex};

use directories::ProjectDirs;
use tracing::{debug, trace, warn};

use upwell_di::{Component, Injectable};

#[cfg(windows)]
mod windows;

/// A kind of application directory. Implemented by the marker types and used by
/// [`DirectoriesManager`] to resolve the concrete path for a [`Dir<Self>`].
///
/// `project_path` resolves against the platform project dirs (with a sensible
/// fallback baked in for kinds that some platforms lack); `rooted_path` is the
/// fallback layout used when no home directory is available.
pub trait DirKind: Send + Sync + 'static {
    /// Name to be displayed when printing the dep graph
    const NAME: &'static str;
    /// Short label — the on-disk subdirectory name and the display name.
    const LABEL: &'static str;
    /// Unique dependency-injection id (namespaced to avoid colliding with user
    /// component ids).
    const COMPONENT_ID: &'static str;
    /// Whether this directory can contain application-private state and must be
    /// created with restrictive permissions.
    const PRIVATE: bool;

    /// Resolves this kind's path from the platform project dirs.
    fn project_path(dirs: &ProjectDirs) -> PathBuf;

    /// Resolves this kind's path under a single fallback root (used when there is no
    /// home directory).
    fn rooted_path(root: &Path) -> PathBuf {
        root.join(Self::LABEL)
    }
}

/// Generates the directory marker types and their [`DirKind`] impls.
macro_rules! dir_kinds {
    ($($(#[$meta:meta])* $name:ident => $label:literal, private = $private:literal, $project:expr;)*) => {
        $(
            $(#[$meta])*
            pub struct $name;

            impl DirKind for $name {
                const NAME: &'static str = concat!(stringify!($name), "Dir");
                const LABEL: &'static str = $label;
                const COMPONENT_ID: &'static str = concat!("upwell:dir:", $label);
                const PRIVATE: bool = $private;

                fn project_path(dirs: &ProjectDirs) -> PathBuf {
                    let resolve: fn(&ProjectDirs) -> PathBuf = $project;

                    resolve(dirs)
                }
            }
        )*
    };
}

dir_kinds! {
    /// Configuration files (`application.toml`, …).
    Config => "config", private = true, |d| d.config_dir().to_path_buf();
    /// Persistent application data.
    Data => "data", private = true, |d| d.data_dir().to_path_buf();
    /// Discardable cached data.
    Cache => "cache", private = true, |d| d.cache_dir().to_path_buf();
    /// State that should persist but is not user data (logs, history).
    State => "state", private = true,
        |d| d.state_dir().unwrap_or_else(|| d.data_dir()).to_path_buf();
    /// Runtime files (sockets, pid files). Platforms without a runtime directory
    /// fall back to cache with an explicit warning.
    Runtime => "runtime", private = true, |d| {
        d.runtime_dir().map(Path::to_path_buf).unwrap_or_else(|| {
            let fallback = d.cache_dir().to_path_buf();
            warn!(
                target: "upwell::dirs",
                path = %fallback.display(),
                "platform runtime directory unavailable; using persistent cache directory"
            );
            fallback
        })
    };
    /// The system temporary directory (shared, not app-scoped).
    Tmp => "tmp", private = false, |_| std::env::temp_dir();
}

/// A resolved application directory of kind `K`, injectable by value.
///
/// Derefs to its [`Path`]. Resolution happened once at the [`DirectoriesManager`], so
/// reading a `Dir<K>` never fails; only creating it on disk ([`ensure`](Self::ensure))
/// performs I/O.
pub struct Dir<K> {
    path: PathBuf,
    secure_root: Option<PathBuf>,
    #[cfg(windows)]
    protection: Arc<Mutex<Vec<windows::PrivateDirectoryGuard>>>,
    _marker: PhantomData<K>,
}

impl<K> Dir<K> {
    /// Wraps an already-resolved path.
    fn new(path: PathBuf, secure_root: Option<PathBuf>) -> Self {
        Self {
            path,
            secure_root,
            #[cfg(windows)]
            protection: Arc::new(Mutex::new(Vec::new())),
            _marker: PhantomData,
        }
    }

    /// The resolved directory path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The path of `name` within this directory.
    pub fn join(&self, name: impl AsRef<Path>) -> PathBuf {
        self.path.join(name)
    }

    /// Creates the directory (and parents) on disk if absent.
    pub fn ensure(&self) -> std::io::Result<()>
    where
        K: DirKind,
    {
        debug!(target: "upwell::dirs", path = %self.path.display(), "ensuring directory exists");

        if !K::PRIVATE {
            return std::fs::create_dir_all(&self.path);
        }

        #[cfg(windows)]
        {
            let mut next = Vec::new();

            if let Some(root) = &self.secure_root {
                next.push(ensure_private_directory(root)?);
            }

            next.push(ensure_private_directory(&self.path)?);
            *self
                .protection
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = next;

            return Ok(());
        }

        #[cfg(not(windows))]
        if let Some(root) = &self.secure_root {
            ensure_private_directory(root)?;
        }

        #[cfg(not(windows))]
        return ensure_private_directory(&self.path);
    }
}

impl<K> Clone for Dir<K> {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            secure_root: self.secure_root.clone(),
            #[cfg(windows)]
            protection: Arc::clone(&self.protection),
            _marker: PhantomData,
        }
    }
}

impl<K> Deref for Dir<K> {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.path
    }
}

#[cfg(unix)]
fn ensure_private_directory(path: &Path) -> std::io::Result<()> {
    use std::fs::{DirBuilder, Permissions};
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};

    const PRIVATE_MODE: u32 = 0o700;
    const UNSAFE_WRITE: u32 = 0o022;
    const STICKY: u32 = 0o1000;

    let effective_uid = unsafe { libc::geteuid() };
    let mut current = PathBuf::new();

    for component in path.components() {
        current.push(component);

        let (metadata, created) = match std::fs::symlink_metadata(&current) {
            Ok(metadata) => (metadata, false),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match DirBuilder::new()
                    .recursive(false)
                    .mode(PRIVATE_MODE)
                    .create(&current)
                {
                    Ok(()) => (std::fs::symlink_metadata(&current)?, true),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        (std::fs::symlink_metadata(&current)?, false)
                    }
                    Err(error) => return Err(error),
                }
            }
            Err(error) => return Err(error),
        };

        let target = current == path;

        if metadata.file_type().is_symlink() {
            // Root-owned platform aliases (for example macOS `/tmp`) are trusted
            // ancestors. The application directory itself is never allowed to be a
            // symlink, and neither are aliases planted by another local user.
            if target || metadata.uid() != 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!(
                        "refusing symlinked application directory: {}",
                        current.display()
                    ),
                ));
            }
        }

        let followed = std::fs::metadata(&current)?;

        if !followed.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotADirectory,
                format!(
                    "application directory path is not a directory: {}",
                    current.display()
                ),
            ));
        }

        if target && followed.uid() != effective_uid {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "application directory is not owned by this user: {}",
                    current.display()
                ),
            ));
        }

        if target && !created && followed.mode() & UNSAFE_WRITE != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "application directory is writable by its group or other users: {}",
                    current.display()
                ),
            ));
        }

        if !target && followed.mode() & UNSAFE_WRITE != 0 && followed.mode() & STICKY == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "application directory has an unsafe writable ancestor: {}",
                    current.display()
                ),
            ));
        }
    }

    std::fs::set_permissions(path, Permissions::from_mode(PRIVATE_MODE))
}

#[cfg(windows)]
fn ensure_private_directory(path: &Path) -> std::io::Result<windows::PrivateDirectoryGuard> {
    windows::ensure_private_directory(path)
}

#[cfg(not(any(unix, windows)))]
fn ensure_private_directory(path: &Path) -> std::io::Result<()> {
    let mut current = PathBuf::new();

    for component in path.components() {
        current.push(component);

        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!(
                        "refusing symlinked application directory: {}",
                        current.display()
                    ),
                ));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotADirectory,
                    format!(
                        "application directory path is not a directory: {}",
                        current.display()
                    ),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&current)?;
            }
            Err(error) => return Err(error),
        }
    }

    Ok(())
}

impl<K: DirKind> Component for Dir<K> {
    type Handle = Dir<K>;

    const ID: &'static str = K::COMPONENT_ID;
    const NAME: &'static str = K::NAME;

    fn into_handle(self) -> Self::Handle {
        self
    }
}

impl<K: DirKind> Injectable for Dir<K> {
    type Target = Dir<K>;
    type Stored = Self;

    fn into_stored(self) -> Self {
        self
    }

    fn from_stored(stored: &Self) -> Self {
        stored.clone()
    }

    fn snapshot_stored(stored: &Self) -> Option<Self> {
        Some(stored.clone())
    }
}

/// Under `di-check`, every `Dir<K>` is framework-seeded, so it is always provided.
#[cfg(feature = "di-check")]
impl<K: DirKind> upwell_di::Provide<Dir<K>> for upwell_di::Wiring {}

/// How a [`DirectoriesManager`] resolves directories.
#[derive(Clone)]
enum Backing {
    /// Platform project directories (XDG / Known Folders).
    Project(ProjectDirs),
    /// A single root all kinds hang off, used when there is no home directory.
    Rooted(PathBuf),
}

/// Resolves and hands out the application's [`Dir<K>`] directories.
///
/// Built once (in `main` or by the daemon builder) from project metadata; cloneable
/// and injectable, so components can resolve arbitrary directory kinds on demand.
#[derive(Clone)]
pub struct DirectoriesManager {
    backing: Backing,
}

impl DirectoriesManager {
    /// Creates a new manager from any path, can be used if nothing else is possible or if default behavior
    /// is not enough. It will never fail.
    pub fn from_path(path: PathBuf) -> Self {
        debug!(target: "upwell::dirs", root = %path.display(), "directories rooted at path");

        Self {
            backing: Backing::Rooted(path),
        }
    }

    /// Resolves directories from project metadata (reverse-DNS `qualifier`,
    /// `organization`, `application`). `None` if no valid home directory exists.
    pub fn from_project(qualifier: &str, organization: &str, application: &str) -> Option<Self> {
        let resolved = ProjectDirs::from(qualifier, organization, application);

        match resolved {
            Some(project) => {
                debug!(target: "upwell::dirs", application, "directories resolved from project metadata");

                Some(Self {
                    backing: Backing::Project(project),
                })
            }

            None => {
                debug!(target: "upwell::dirs", application, "no home directory; project directories unavailable");

                None
            }
        }
    }

    /// A fail-closed manager for `application` using platform project directories.
    ///
    /// Panics when the platform cannot resolve a home directory. Framework builders
    /// use [`try_for_app`](Self::try_for_app) and return an error instead; applications
    /// that intentionally need another layout should pass [`from_path`](Self::from_path).
    pub fn for_app(application: &str) -> Self {
        Self::try_for_app(application).expect(
            "no safe application directories are available; supply an explicit DirectoriesManager::from_path",
        )
    }

    /// Resolves safe platform project directories, refusing to fall back to a
    /// predictable path in the shared system temporary directory.
    pub fn try_for_app(application: &str) -> std::io::Result<Self> {
        Self::from_project("", "", application).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no home directory is available for safe application directories",
            )
        })
    }

    /// Resolves the directory of kind `K`.
    pub fn dir<K: DirKind>(&self) -> Dir<K> {
        let (path, secure_root) = match &self.backing {
            Backing::Project(project) => (K::project_path(project), None),
            Backing::Rooted(root) => (K::rooted_path(root), Some(root.clone())),
        };

        trace!(target: "upwell::dirs", kind = K::LABEL, path = %path.display(), "resolved directory");

        Dir::new(path, secure_root)
    }

    /// The platform's config directory path — where `application.<ext>` files live. A
    /// convenience for locating config sources without naming the [`Config`] kind.
    pub fn config_path(&self) -> PathBuf {
        self.dir::<Config>().path().to_path_buf()
    }

    /// Every known directory kind as a `(label, resolved path)` pair, in a stable order.
    ///
    /// This is the directory namespace as plain data: the config crate inspects it to build
    /// a `${@<label>}` templating resolver, so config never names the individual `Dir` kind
    /// types — only this list. Labels are the [`DirKind::LABEL`]s: `config`, `data`, `cache`,
    /// `state`, `runtime`, `tmp`.
    pub fn entries(&self) -> Vec<(&'static str, PathBuf)> {
        let entries = vec![
            (Config::LABEL, self.dir::<Config>().path().to_path_buf()),
            (Data::LABEL, self.dir::<Data>().path().to_path_buf()),
            (Cache::LABEL, self.dir::<Cache>().path().to_path_buf()),
            (State::LABEL, self.dir::<State>().path().to_path_buf()),
            (Runtime::LABEL, self.dir::<Runtime>().path().to_path_buf()),
            (Tmp::LABEL, self.dir::<Tmp>().path().to_path_buf()),
        ];

        debug!(target: "upwell::dirs", kinds = entries.len(), "enumerated directory namespace");

        entries
    }
}

impl Component for DirectoriesManager {
    type Handle = DirectoriesManager;

    const ID: &'static str = "upwell:directories";
    const NAME: &'static str = "DirectoriesManager";

    fn into_handle(self) -> Self::Handle {
        self
    }
}

impl Injectable for DirectoriesManager {
    type Target = DirectoriesManager;
    type Stored = Self;

    fn into_stored(self) -> Self {
        self
    }

    fn from_stored(stored: &Self) -> Self {
        stored.clone()
    }

    fn snapshot_stored(stored: &Self) -> Option<Self> {
        Some(stored.clone())
    }
}

/// Under `di-check`, the manager is framework-seeded, so it is always provided.
#[cfg(feature = "di-check")]
impl upwell_di::Provide<DirectoriesManager> for upwell_di::Wiring {}

#[cfg(test)]
mod tests;
