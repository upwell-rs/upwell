//! One scope named through two paths in a scope list passes the macro, which compares the
//! written paths, and fails registry resolution, which compares the scope keys.
#![allow(dead_code)]

use upwell::{ComponentRegistry, DiError, Scope, ScopeId, component};

/// The only scope of the list, named twice.
struct Local;

impl Scope for Local {
    const ID: ScopeId = upwell::namespaced_id!(ScopeId, "test/repeated-path");
    const RANK: u8 = 1;
    const NAME: &'static str = "Local";
}

mod alias {
    pub(crate) use super::Local as Same;
}

/// A component listing [`Local`] under two paths.
#[component(scope = [Local, alias::Same])]
struct Twice {}

#[test]
fn a_scope_named_through_two_paths_fails_resolution() {
    let error = ComponentRegistry::collect()
        .resolved_components()
        .expect_err("one scope listed twice is rejected");

    assert!(
        matches!(
            &error,
            DiError::RepeatedScope(repeated)
                if repeated.component == "Twice"
                    && repeated.scope == "Local"
                    && repeated.factory.is_none()
        ),
        "{error}"
    );
}
