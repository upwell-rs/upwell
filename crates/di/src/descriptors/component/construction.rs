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
    /// Never built: a `#[context]` value, which the protocol or the application seeds into
    /// an instance of each scope it lists. Unlike any other factory-less component, it may be
    /// placed in several scopes that share no hierarchy, and preparation checks that a seed
    /// claim covers each placement something depends on.
    Context,
}

impl ScopedConstruction {
    /// The policy's stable lowercase name, as tooling reports it.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OnDemand => "on-demand",
            Self::Eager => "eager",
            Self::Context => "context",
        }
    }

    /// Whether every opening of the scope builds the component.
    pub const fn is_eager(self) -> bool {
        matches!(self, Self::Eager)
    }

    /// Whether the component is a `#[context]` value, seeded into each scope it lists.
    pub const fn is_context(self) -> bool {
        matches!(self, Self::Context)
    }
}
