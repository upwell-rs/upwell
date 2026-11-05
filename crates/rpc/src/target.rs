//! The invocation target the RPC protocol establishes contexts for.

use upwell_di::context::InvocationTarget;

/// One RPC call, middleware, handler and response stream included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RpcTarget;

impl InvocationTarget for RpcTarget {}
