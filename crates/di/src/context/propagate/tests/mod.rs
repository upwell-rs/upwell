//! Propagation of the invocation context into spawned work.

use std::sync::Arc;

use super::*;
use crate::container::ScopeContainer;
use crate::context::fixtures::{Chain, Request, chain, child, context, innermost};
use crate::context::{ContextError, Invocation};

mod async_tasks;
#[cfg(feature = "rayon")]
mod rayon;
mod sync;

/// Captures a context running in a fresh request instance of `chain`, with that instance.
async fn captured(chain: &Chain) -> (CapturedContext, Arc<ScopeContainer>) {
    let request = child::<Request>(&chain.root).await;
    let captured = context(chain, Invocation::HttpRequest)
        .with_scope(Arc::clone(&request))
        .enter(async { CapturedContext::capture().expect("the context is active") })
        .await;

    (captured, request)
}

/// Whether the active context runs in `expected`.
fn sees(expected: &Arc<ScopeContainer>) -> bool {
    innermost().is_ok_and(|scope| Arc::ptr_eq(&scope, expected))
}

/// Whether no context is active.
fn sees_nothing() -> bool {
    matches!(innermost(), Err(ContextError::OutsideContext))
}
