use upwell_core::{DependencyDescriptor, EntryKey, ItemSource, Scope, ScopeKey, TypeDescriptor};
use upwell_di::FromContainer;

/// The type and operation that own an entry, such as a controller and its handler method.
#[derive(Clone, Copy, Debug)]
pub struct EntryOwner {
    /// The owning type.
    pub ty: TypeDescriptor,
    /// The owner's operation, such as the handler method name.
    pub operation: &'static str,
}

/// One registered entry point: the scope it runs in and the targets it injects.
///
/// Protocols, plugins and the application builder register entries. Preparation validates
/// every target against the entry's scope and assigns the entry a dense
/// [`EntryId`](super::EntryId).
#[derive(Clone, Copy, Debug)]
pub struct EntryDescriptor {
    /// The stable entry identity.
    pub key: EntryKey,
    /// The scope the entry runs in.
    pub scope: ScopeKey,
    /// The dependency edges of the injected targets, in parameter order.
    pub targets: fn() -> Vec<DependencyDescriptor>,
    /// The type and operation owning the entry, when it has one.
    pub owner: Option<EntryOwner>,
    /// The entry's declaration, when the registering macro recorded it.
    pub source: Option<ItemSource>,
}

impl EntryDescriptor {
    /// An entry into scope `S` injecting the targets `T`.
    pub const fn new<S: Scope, T: EntryTargets>(key: EntryKey) -> Self {
        Self {
            key,
            scope: S::KEY,
            targets: T::targets,
            owner: None,
            source: None,
        }
    }

    /// Records the type and operation owning this entry.
    pub const fn owned_by(mut self, owner: EntryOwner) -> Self {
        self.owner = Some(owner);

        self
    }

    /// Records the entry's declaration.
    pub const fn declared_at(mut self, source: ItemSource) -> Self {
        self.source = Some(source);

        self
    }
}

/// The injected targets of an entry, written as a tuple of injectable parameter types.
///
/// Implemented for `()` and for tuples of up to twelve [`FromContainer`] types, so a tuple
/// declares the same dependency edges its types would declare as factory parameters.
pub trait EntryTargets {
    /// The dependency edge of every target, in tuple order.
    fn targets() -> Vec<DependencyDescriptor>;
}

impl EntryTargets for () {
    fn targets() -> Vec<DependencyDescriptor> {
        Vec::new()
    }
}

macro_rules! entry_targets {
    ($($target:ident),+) => {
        impl<$($target: FromContainer),+> EntryTargets for ($($target,)+) {
            fn targets() -> Vec<DependencyDescriptor> {
                vec![$(<$target as FromContainer>::dependency()),+]
            }
        }
    };
}

entry_targets!(A);
entry_targets!(A, B);
entry_targets!(A, B, C);
entry_targets!(A, B, C, D);
entry_targets!(A, B, C, D, E);
entry_targets!(A, B, C, D, E, F);
entry_targets!(A, B, C, D, E, F, G);
entry_targets!(A, B, C, D, E, F, G, H);
entry_targets!(A, B, C, D, E, F, G, H, I);
entry_targets!(A, B, C, D, E, F, G, H, I, J);
entry_targets!(A, B, C, D, E, F, G, H, I, J, K);
entry_targets!(A, B, C, D, E, F, G, H, I, J, K, L);
