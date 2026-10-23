//! Generation snapshots: how a retained singleton's instance carries across runtime generations.

use super::{BoxedComponent, Component, Injectable, LiveCell};

/// Hidden typed generation adapter: the optional [`Retention`] that carries an instance
/// into a later generation, plus the live-cell accessor that reads a stored slot's
/// [`Live<T>`] cell.
///
/// Live-cell access only needs the component's type, so it stays available when a
/// descriptor opts out of retention: live consumers of a never-retained component can
/// still be rebound in place.
#[derive(Clone, Copy)]
#[doc(hidden)]
pub struct GenerationSnapshot {
    retention: Option<Retention>,
    live: fn(&BoxedComponent) -> Option<LiveCell>,
}

/// The typed snapshot adapter that carries an instance into a later generation, and the
/// validator for the payload it produces.
#[derive(Clone, Copy)]
pub(crate) struct Retention {
    snapshot: fn(&BoxedComponent) -> crate::Result<BoxedComponent>,
    validate: fn(&BoxedComponent) -> bool,
}

impl Retention {
    pub(crate) fn snapshot(self, active: &BoxedComponent) -> crate::Result<BoxedComponent> {
        (self.snapshot)(active)
    }

    pub(crate) fn validates(self, candidate: &BoxedComponent) -> bool {
        (self.validate)(candidate)
    }
}

impl GenerationSnapshot {
    /// The typed snapshot of `T`: retained across generations, with live access.
    pub(super) const fn of<T: Component>() -> Self {
        Self {
            retention: Some(Retention {
                snapshot: generation_snapshot_adapter::<T>,
                validate: generation_snapshot_validates::<T>,
            }),
            live: generation_live_cell::<T>,
        }
    }

    /// The adapter that carries the instance into a later generation, unless the
    /// descriptor opted out of retention.
    pub(crate) fn retention(self) -> Option<Retention> {
        self.retention
    }

    pub(crate) fn live_cell(self, stored: &BoxedComponent) -> Option<LiveCell> {
        (self.live)(stored)
    }

    /// This adapter without retention, keeping its live-cell access.
    pub(crate) const fn without_retention(self) -> Self {
        Self {
            retention: None,
            ..self
        }
    }

    #[cfg(test)]
    pub(crate) const fn for_test(
        snapshot: fn(&BoxedComponent) -> crate::Result<BoxedComponent>,
        validate: fn(&BoxedComponent) -> bool,
    ) -> Self {
        Self {
            retention: Some(Retention { snapshot, validate }),
            live: |_| None,
        }
    }
}

/// The generic generation-snapshot adapter for a typed component: recovers the
/// handle from the active stored slot and boxes a *new* stored representation.
/// For an `Arc<T>` handle this shares the same `Arc` instance but creates an
/// independent [`Live<T>`] cell. By-value handles work only when their `Injectable`
/// implementation explicitly opts in (the component macro's `retainable` flag).
pub(crate) fn generation_snapshot_adapter<T: Component>(
    active: &BoxedComponent,
) -> crate::Result<BoxedComponent> {
    let stored = active
        .value
        .downcast_ref::<<T::Handle as Injectable>::Stored>()
        .ok_or(crate::Error::SnapshotStorageMismatch { component: T::ID })?;

    let candidate_stored = <T::Handle as Injectable>::snapshot_stored(stored)
        .ok_or(crate::Error::SnapshotHandleUnsupported { component: T::ID })?;

    Ok(BoxedComponent {
        ty: active.ty,
        value: Box::new(candidate_stored),
    })
}

/// The live cell a typed component's stored slot holds, when its handle stores one.
fn generation_live_cell<T: Component>(stored: &BoxedComponent) -> Option<LiveCell> {
    stored
        .value
        .downcast_ref::<<T::Handle as Injectable>::Stored>()
        .and_then(<T::Handle as Injectable>::live_cell)
}

fn generation_snapshot_validates<T: Component>(candidate: &BoxedComponent) -> bool {
    candidate
        .value
        .downcast_ref::<<T::Handle as Injectable>::Stored>()
        .is_some()
}
