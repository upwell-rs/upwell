//! Type-erased component instances and the per-scope stores that hold them.

mod boxed;
mod keyed;
mod scope_store;
mod slots;

pub use boxed::{BoxedComponent, from_boxed};
pub(crate) use keyed::KeyedStore;
pub(crate) use scope_store::ScopeStore;
pub(crate) use slots::SlotStore;
