//! The live slot primitive behind `Dep<T>`, and the type-erased cell handles runtime
//! transitions use to republish the slots retained consumers hold.

use std::any::Any;
use std::fmt;
use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, Weak};

use arc_swap::{ArcSwap, Guard};

use super::Injectable;

/// A shared, swappable cell holding the current `Arc<T>` instance — the interior
/// mutability primitive behind [`Dep<T>`] (and `Cfg<T>` in the config layer).
///
/// Backed by [`arc_swap`]: every clone shares one underlying `ArcSwap`, so a
/// [`replace`](Self::replace) is observed by all clones, while [`snapshot`](Self::snapshot)
/// and [`get`](Self::get) pin the generation current at the call. This is what lets a
/// config reload swap a component (or config value) in place without recreating its
/// consumers.
pub struct Live<T: ?Sized> {
    inner: Slot<T>,
}

/// What a [`Live<T>`] holds: a swappable cell, a root's slot over one, or a fixed instance
/// stored in a scope below the root, which no transition ever rebinds.
enum Slot<T: ?Sized> {
    Swappable(Arc<SwapCell<T>>),
    Rooted(Arc<RootSlot<T>>),
    Fixed(Arc<T>),
}

/// The cell behind a swappable [`Live<T>`], whether a transition has republished it, and the
/// root slot it was created for, if any.
struct SwapCell<T: ?Sized> {
    current: ArcSwap<Arc<T>>,
    rebound: AtomicBool,
    root: Option<Weak<RootSlot<T>>>,
}

/// A root's slot: the instance the root holds, and the swappable cell `Dep<T>` handles share,
/// created when the first one is read.
///
/// A retained consumer's `Dep<T>` shares the cell of the root that built it, so a later
/// transition republishes the cell of a root it displaced. Reads through that root's storage
/// then keep returning the root's own instance, so the displaced generation, and the scopes
/// still open on it, never see a later generation's instance. That is the instance the cell
/// held when the transition rebound it, so a `Dep::replace` made before it stays visible. The
/// root holds both instances and the cell only refers back to the root, so they are released
/// with the root, not with the consumer holding the cell.
struct RootSlot<T: ?Sized> {
    instance: Arc<T>,
    rebound_instance: OnceLock<Arc<T>>,
    cell: OnceLock<Arc<SwapCell<T>>>,
}

impl<T: ?Sized> RootSlot<T> {
    fn cell(self: &Arc<Self>) -> &Arc<SwapCell<T>> {
        self.cell
            .get_or_init(|| SwapCell::rooted(Arc::clone(&self.instance), Arc::downgrade(self)))
    }

    /// The instance reads through this root return once its cell is rebound.
    fn own_instance(&self) -> Arc<T> {
        Arc::clone(self.rebound_instance.get().unwrap_or(&self.instance))
    }
}

impl<T: ?Sized> SwapCell<T> {
    fn new(value: Arc<T>) -> Arc<Self> {
        Self::with_root(value, None)
    }

    fn rooted(value: Arc<T>, root: Weak<RootSlot<T>>) -> Arc<Self> {
        Self::with_root(value, Some(root))
    }

    fn with_root(value: Arc<T>, root: Option<Weak<RootSlot<T>>>) -> Arc<Self> {
        Arc::new(Self {
            current: ArcSwap::new(Arc::new(value)),
            rebound: AtomicBool::new(false),
            root,
        })
    }
}

/// A reloadable dependency handle: a stable handle to a component slot whose
/// instance may be swapped at runtime (e.g. by a config reload).
///
/// A `Dep<T>` field keeps working across a swap, observing the new instance on its
/// next read; existing snapshots keep the old instance alive until dropped. It is
/// the same primitive as [`Live<T>`] — an alias chosen at the injection site to opt
/// into live, rather than fixed `Arc<T>`, semantics.
pub type Dep<T> = Live<T>;

/// A guard pinning one generation of a [`Live<T>`], dereferencing to `T`. Holding it
/// keeps observing the generation current when [`get`](Live::get) was called; it
/// borrows the `Live`, discouraging holding it across a long await (which would pin
/// the old instance and delay a reload). Returned by both [`Dep<T>`] and `Cfg<T>`.
pub struct LiveRef<'a, T: ?Sized> {
    pinned: Pinned<'a, T>,
}

/// The instance a [`LiveRef`] pins.
enum Pinned<'a, T: ?Sized> {
    Guard(Guard<Arc<Arc<T>>>, PhantomData<&'a Live<T>>),
    Fixed(&'a T),
}

impl<T: ?Sized> Deref for LiveRef<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        match &self.pinned {
            Pinned::Guard(guard, _) => guard,
            Pinned::Fixed(value) => value,
        }
    }
}

impl<T: ?Sized + Send + Sync + 'static> Clone for Live<T> {
    fn clone(&self) -> Self {
        let inner = match &self.inner {
            Slot::Swappable(cell) => Slot::Swappable(Arc::clone(cell)),
            Slot::Rooted(slot) => Slot::Rooted(Arc::clone(slot)),
            Slot::Fixed(value) => Slot::Fixed(Arc::clone(value)),
        };

        Self { inner }
    }
}

impl<T: ?Sized + Send + Sync + 'static> fmt::Debug for Live<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Live").finish_non_exhaustive()
    }
}

impl<T: ?Sized + Send + Sync + 'static> Live<T> {
    /// Wraps an instance in a fresh slot. Each call creates an independent cell, so
    /// this is the seam where a constructed instance becomes reloadable.
    pub fn new(value: Arc<T>) -> Self {
        Self {
            inner: Slot::Swappable(SwapCell::new(value)),
        }
    }

    /// Holds a singleton in a root's slot, whose instance stays the root's own after a
    /// transition republishes the cell its `Dep<T>` handles share.
    pub(crate) fn rooted(value: Arc<T>) -> Self {
        Self {
            inner: Slot::Rooted(Arc::new(RootSlot {
                instance: value,
                rebound_instance: OnceLock::new(),
                cell: OnceLock::new(),
            })),
        }
    }

    /// Holds an instance stored in a scope below the root. Nothing republishes such an
    /// instance, so it needs no cell: reads clone the `Arc`.
    pub(crate) fn fixed(value: Arc<T>) -> Self {
        Self {
            inner: Slot::Fixed(value),
        }
    }

    /// Publishes a new instance into the slot. Every clone of this `Live` (every
    /// `Dep<T>` derived from it) observes the new value on its next read — the
    /// swap a config reload performs at commit.
    ///
    /// Every `Live` a caller can obtain is swappable: a `Dep<T>` read from a fixed slot
    /// gets its own cell. Replacing a fixed slot's own storage is a container bug, which
    /// debug builds report and release builds ignore.
    pub fn replace(&self, value: Arc<T>) {
        match self.swap_cell() {
            Some(cell) => cell.current.store(Arc::new(value)),
            None => debug_assert!(false, "a fixed slot's storage was replaced"),
        }
    }

    /// Publishes a later generation's instance into a cell a transition rebinds, so the root
    /// slot over it keeps reading the instance the cell held until then.
    ///
    /// The root keeps that instance before the flag is set, so a read that sees the flag also
    /// sees it.
    fn rebind(&self, value: Arc<T>) {
        if let Some(cell) = self.swap_cell() {
            if let Some(root) = cell.root.as_ref().and_then(Weak::upgrade) {
                root.rebound_instance
                    .get_or_init(|| cell.current.load_full().as_ref().clone());
            }

            cell.rebound.store(true, Ordering::SeqCst);
        }

        self.replace(value);
    }

    fn swap_cell(&self) -> Option<&Arc<SwapCell<T>>> {
        match &self.inner {
            Slot::Swappable(cell) => Some(cell),
            Slot::Rooted(slot) => Some(slot.cell()),
            Slot::Fixed(_) => None,
        }
    }

    /// The instance a scope storing this slot holds. A root's slot reads its cell until a
    /// transition republishes the cell, and its own instance from then on.
    ///
    /// The cell is read before the flag: a rebind sets the flag before it stores, so a read
    /// that saw the later instance also sees the flag.
    fn stored_instance(&self) -> Arc<T> {
        match &self.inner {
            Slot::Rooted(slot) => {
                let Some(cell) = slot.cell.get() else {
                    return Arc::clone(&slot.instance);
                };
                let current = cell.current.load_full().as_ref().clone();

                match cell.rebound.load(Ordering::SeqCst) {
                    true => slot.own_instance(),
                    false => current,
                }
            }
            Slot::Swappable(_) | Slot::Fixed(_) => self.snapshot(),
        }
    }

    /// An owned `Arc` snapshot of the current instance — stable once taken. Prefer
    /// this over [`get`](Self::get) for anything held across an `.await`.
    pub fn snapshot(&self) -> Arc<T> {
        match &self.inner {
            Slot::Swappable(cell) => cell.current.load_full().as_ref().clone(),
            Slot::Rooted(slot) => match slot.cell.get() {
                Some(cell) => cell.current.load_full().as_ref().clone(),
                None => Arc::clone(&slot.instance),
            },
            Slot::Fixed(value) => Arc::clone(value),
        }
    }

    /// A guard pinning the current instance, dereferencing to `T`, for short
    /// synchronous reads.
    pub fn get(&self) -> LiveRef<'_, T> {
        let pinned = match &self.inner {
            Slot::Swappable(cell) => Pinned::Guard(cell.current.load(), PhantomData),
            Slot::Rooted(slot) => match slot.cell.get() {
                Some(cell) => Pinned::Guard(cell.current.load(), PhantomData),
                None => Pinned::Fixed(slot.instance.as_ref()),
            },
            Slot::Fixed(value) => Pinned::Fixed(value.as_ref()),
        };

        LiveRef { pinned }
    }

    /// A `Live` of the same kind as this one, holding `value`: how a provider alias keeps
    /// the storage kind of the instance it aliases.
    pub(crate) fn with_value<U: ?Sized + Send + Sync + 'static>(&self, value: Arc<U>) -> Live<U> {
        match &self.inner {
            Slot::Swappable(_) => Live::new(value),
            Slot::Rooted(_) => Live::rooted(value),
            Slot::Fixed(_) => Live::fixed(value),
        }
    }

    /// A handle sharing this slot's cell. A fixed slot has none, so the handle gets a
    /// cell of its own, which nothing republishes either.
    fn share(&self) -> Self {
        match &self.inner {
            Slot::Swappable(_) => self.clone(),
            Slot::Rooted(slot) => Self {
                inner: Slot::Swappable(Arc::clone(slot.cell())),
            },
            Slot::Fixed(value) => Self::new(Arc::clone(value)),
        }
    }

    /// The handle a transition republishes, when this slot is swappable.
    fn cell(&self) -> Option<LiveCell> {
        match &self.inner {
            Slot::Swappable(_) | Slot::Rooted(_) => Some(LiveCell::of(&self.share())),
            Slot::Fixed(_) => None,
        }
    }
}

impl<T: ?Sized + Send + Sync + 'static> Injectable for Dep<T> {
    type Target = T;
    type Stored = Live<T>;

    fn into_stored(self) -> Live<T> {
        self
    }

    fn from_stored(stored: &Live<T>) -> Self {
        stored.share()
    }

    fn live_cell(stored: &Live<T>) -> Option<LiveCell> {
        stored.cell()
    }
}

impl<T: ?Sized + Send + Sync + 'static> Injectable for Arc<T> {
    type Target = T;
    type Stored = Live<T>;

    fn into_stored(self) -> Live<T> {
        Live::rooted(self)
    }

    fn into_fixed(self) -> Live<T> {
        Live::fixed(self)
    }

    fn from_stored(stored: &Live<T>) -> Self {
        stored.stored_instance()
    }

    fn snapshot_stored(stored: &Live<T>) -> Option<Live<T>> {
        Some(Live::rooted(stored.stored_instance()))
    }

    fn live_cell(stored: &Live<T>) -> Option<LiveCell> {
        stored.cell()
    }
}

/// A type-erased handle onto one shared [`Live<T>`] cell.
///
/// Runtime transitions use it to republish the exact cell a retained consumer's
/// `Dep<T>` field shares, without knowing `T`. Cloning the handle shares the cell.
#[doc(hidden)]
#[derive(Clone)]
pub struct LiveCell {
    cell: Arc<dyn ErasedLive>,
}

/// One prepared, infallible republication of a [`LiveCell`].
///
/// Preparing it is the only fallible step; [`commit`](Self::commit) only swaps the
/// cell's value, so it can run inside a terminal publication. Dropping it without
/// committing leaves the cell untouched.
#[doc(hidden)]
pub struct LiveRebind {
    commit: Box<dyn FnOnce() + Send>,
}

/// The typed operations a [`LiveCell`] erases.
trait ErasedLive: Send + Sync {
    fn as_any(&self) -> &dyn Any;

    fn rebind_from(&self, source: &dyn Any) -> Option<LiveRebind>;

    fn cell_ptr(&self) -> *const ();
}

impl<T: ?Sized + Send + Sync + 'static> ErasedLive for Live<T> {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn rebind_from(&self, source: &dyn Any) -> Option<LiveRebind> {
        let value = source.downcast_ref::<Live<T>>()?.snapshot();
        let cell = self.cell().map(|_| self.clone())?;

        Some(LiveRebind {
            commit: Box::new(move || cell.rebind(value)),
        })
    }

    fn cell_ptr(&self) -> *const () {
        match &self.inner {
            Slot::Swappable(cell) => Arc::as_ptr(cell).cast(),
            Slot::Rooted(slot) => Arc::as_ptr(slot.cell()).cast(),
            Slot::Fixed(value) => Arc::as_ptr(value).cast(),
        }
    }
}

impl LiveCell {
    /// A handle onto the cell `live` shares.
    pub fn of<T: ?Sized + Send + Sync + 'static>(live: &Live<T>) -> Self {
        Self {
            cell: Arc::new(live.clone()),
        }
    }

    /// Prepares republishing this cell with the value `source` currently holds.
    ///
    /// `None` when the two cells hold different types.
    pub fn rebind_to(&self, source: &LiveCell) -> Option<LiveRebind> {
        self.cell.rebind_from(source.cell.as_any())
    }

    /// Whether both handles share one underlying cell.
    pub fn same_cell(&self, other: &LiveCell) -> bool {
        std::ptr::eq(self.cell.cell_ptr(), other.cell.cell_ptr())
    }
}

impl fmt::Debug for LiveCell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LiveCell").finish_non_exhaustive()
    }
}

impl LiveRebind {
    /// Publishes the prepared value into the cell.
    pub fn commit(self) {
        (self.commit)();
    }
}

impl fmt::Debug for LiveRebind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LiveRebind").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
