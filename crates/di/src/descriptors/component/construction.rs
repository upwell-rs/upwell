/// When a component of a scope below the root is built in an instance of its scope.
///
/// Singletons are built when the application starts and transients on every resolution, so
/// the policy only applies to components of other scopes; validation rejects
/// [`Eager`](Self::Eager) elsewhere.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum ScopedConstruction {
    /// Built when an entry's plan lists it or when something first resolves it from the
    /// open scope.
    #[default]
    OnDemand,
    /// Built on every opening of the scope, whatever the entry, for a component whose
    /// construction is the intended side effect (an audit record, a span, a gate).
    Eager,
}

impl ScopedConstruction {
    /// The policy's stable lowercase name, as tooling reports it.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OnDemand => "on-demand",
            Self::Eager => "eager",
        }
    }

    /// Whether every opening of the scope builds the component.
    pub const fn is_eager(self) -> bool {
        matches!(self, Self::Eager)
    }
}
