//! The boxed detail payloads of the scope-list errors.

use std::fmt;

use upwell_core::ScopeId;

/// A component whose non-empty scope list does not start with its declared scope.
#[derive(Debug)]
pub struct DeclaredScopeMismatch {
    /// Human-readable component name.
    pub component: String,
    /// Stable component identity.
    pub component_id: String,
    /// The declared scope's name.
    pub scope: &'static str,
    /// The declared scope's identity.
    pub scope_id: ScopeId,
    /// The name of the scope the list starts with.
    pub first: &'static str,
    /// The identity of the scope the list starts with.
    pub first_id: ScopeId,
}

impl fmt::Display for DeclaredScopeMismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "component '{}' declares scope '{}' but lists '{}' first; a component that lists \
             several scopes declares the first of them",
            self.component, self.scope, self.first
        )
    }
}

/// A component or factory scope list that names one scope twice.
#[derive(Debug)]
pub struct RepeatedScope {
    /// Human-readable component name.
    pub component: String,
    /// Stable component identity.
    pub component_id: String,
    /// The factory whose list repeats the scope, or `None` for the component's own list.
    pub factory: Option<&'static str>,
    /// The repeated scope's name.
    pub scope: &'static str,
    /// The repeated scope's identity.
    pub scope_id: ScopeId,
}

impl fmt::Display for RepeatedScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "component '{}' lists scope '{}' twice",
            self.component, self.scope
        )?;

        if let Some(factory) = self.factory {
            write!(formatter, " for factory '{factory}'")?;
        }

        write!(formatter, "; each scope builds the component once")
    }
}
