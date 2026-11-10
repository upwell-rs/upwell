//! The runtime an invocation context belongs to, opaque to the DI layer.

use std::any::Any;
use std::sync::Arc;

use crate::container::ScopeContainer;

/// The application runtime an invocation context belongs to, opaque to the DI layer.
#[doc(hidden)]
pub trait ContextRuntime: Any + Send + Sync {
    /// The root of the generation a context without an open scope resolves from.
    fn root(&self) -> Option<Arc<ScopeContainer>>;
}
