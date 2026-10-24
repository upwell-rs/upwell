//! Scope-capturing deferred and forced-construction dependency handles.

mod deferred;
mod fresh;
mod lazy;

pub use deferred::Deferred;
pub(crate) use deferred::DeferredHydrator;
pub use fresh::{Fresh, FreshFromContainer};
pub use lazy::Lazy;

#[cfg(test)]
mod tests;
