//! The type-erased storage of one instance, held inline when it fits.

use std::any::{Any, TypeId};
use std::cell::UnsafeCell;
use std::mem::{ManuallyDrop, MaybeUninit, align_of, size_of};

/// Inline room for one stored value, sized for the largest common handle: a `Live<dyn Trait>`,
/// which is a tag and a wide pointer.
type InlineBytes = MaybeUninit<[usize; 3]>;

/// A type-erased stored value: what a scope holds for one instance.
///
/// A value that fits in three words, as every `Live<T>` an `Arc<T>` or `Dep<T>` handle stores
/// does, is held inline, so storing an `Arc<T>` costs only the `Arc`'s own allocation. A larger
/// or more strictly aligned value is boxed.
pub struct StoredValue {
    repr: Repr,
}

/// Where a [`StoredValue`] keeps its value.
enum Repr {
    Inline(InlineValue),
    Boxed(Box<dyn Any + Send + Sync>),
}

/// A value of the type `ops` describes, held in `bytes`.
///
/// The bytes sit in an [`UnsafeCell`], so a value with interior mutability (an atomic, a
/// `Mutex`) may be written through the `&S` [`downcast_ref`](Self::downcast_ref) returns: plain
/// bytes would be treated as immutable behind a shared reference.
struct InlineValue {
    bytes: UnsafeCell<InlineBytes>,
    ops: &'static InlineOps,
}

// SAFETY: [`StoredValue::new`], the only constructor, requires the value's type to be `Send`.
#[allow(unsafe_code)]
unsafe impl Send for InlineValue {}

// SAFETY: [`StoredValue::new`], the only constructor, requires the value's type to be `Sync`,
// and an `InlineValue` hands out only `&S` through a shared reference.
#[allow(unsafe_code)]
unsafe impl Sync for InlineValue {}

/// What an [`InlineValue`] needs to know about the type it erases.
struct InlineOps {
    type_id: TypeId,
    drop: unsafe fn(*mut InlineBytes),
}

impl StoredValue {
    /// Stores `value`, inline when it fits.
    pub fn new<S: Any + Send + Sync>(value: S) -> Self {
        let repr = match size_of::<S>() <= size_of::<InlineBytes>()
            && align_of::<S>() <= align_of::<InlineBytes>()
        {
            true => Repr::Inline(InlineValue::new(value)),
            false => Repr::Boxed(Box::new(value)),
        };

        Self { repr }
    }

    /// The value, when it is an `S`.
    pub fn downcast_ref<S: Any>(&self) -> Option<&S> {
        match &self.repr {
            Repr::Inline(inline) => inline.downcast_ref::<S>(),
            Repr::Boxed(boxed) => boxed.downcast_ref::<S>(),
        }
    }

    /// Whether the value is an `S`.
    pub fn is<S: Any>(&self) -> bool {
        self.downcast_ref::<S>().is_some()
    }

    /// Takes the value out when it is an `S`, and hands this storage back otherwise.
    pub fn downcast<S: Any>(self) -> Result<S, Self> {
        match self.repr {
            Repr::Inline(inline) => inline.take::<S>().map_err(|inline| Self {
                repr: Repr::Inline(inline),
            }),

            Repr::Boxed(boxed) => boxed
                .downcast::<S>()
                .map(|boxed| *boxed)
                .map_err(|boxed| Self {
                    repr: Repr::Boxed(boxed),
                }),
        }
    }
}

impl InlineValue {
    /// Moves `value` into inline storage. [`StoredValue::new`] checks it fits.
    fn new<S: Any + Send + Sync>(value: S) -> Self {
        let mut bytes = InlineBytes::uninit();

        // SAFETY: the caller checked `S` fits in `InlineBytes` by size and alignment, and
        // `bytes` is valid for writes. `MaybeUninit` keeps every byte of the value, provenance
        // included, whenever this struct moves.
        #[allow(unsafe_code)]
        unsafe {
            bytes.as_mut_ptr().cast::<S>().write(value)
        };

        Self {
            bytes: UnsafeCell::new(bytes),
            ops: const {
                &InlineOps {
                    type_id: TypeId::of::<S>(),
                    drop: drop_inline::<S>,
                }
            },
        }
    }

    #[allow(unsafe_code)]
    fn downcast_ref<S: Any>(&self) -> Option<&S> {
        // SAFETY: equal type ids mean `new` wrote an `S` into `bytes`, and it stays initialized
        // until this value is dropped or taken, both of which consume it.
        (self.ops.type_id == TypeId::of::<S>()).then(|| unsafe { &*self.bytes.get().cast::<S>() })
    }

    #[allow(unsafe_code)]
    fn take<S: Any>(self) -> Result<S, Self> {
        if self.ops.type_id != TypeId::of::<S>() {
            return Err(self);
        }

        let this = ManuallyDrop::new(self);

        // SAFETY: equal type ids mean `new` wrote an `S` into `bytes`. `ManuallyDrop` keeps it
        // from being dropped here as well as read out.
        Ok(unsafe { this.bytes.get().cast::<S>().read() })
    }
}

impl Drop for InlineValue {
    fn drop(&mut self) {
        // SAFETY: `ops` was made for the type `new` wrote into `bytes`, which is initialized
        // and is dropped only here.
        #[allow(unsafe_code)]
        unsafe {
            (self.ops.drop)(self.bytes.get_mut())
        };
    }
}

/// Drops the `S` stored in `bytes`.
///
/// # Safety
///
/// `bytes` must point to a live `S` written by [`InlineValue::new`], which must not be used
/// again.
#[allow(unsafe_code)]
unsafe fn drop_inline<S>(bytes: *mut InlineBytes) {
    // SAFETY: the caller upholds that `bytes` holds a live `S` that nothing reads again.
    unsafe { bytes.cast::<S>().drop_in_place() };
}

#[cfg(test)]
mod tests;
