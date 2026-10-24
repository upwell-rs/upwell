//! Type-erased component instances and the per-scope store that holds them.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::ops::Deref;

use upwell_core::TypeDescriptor;

use super::{Injectable, ProviderDescriptor};

/// A type-erased instantiated component.
///
/// `value` stores `Arc<T>` inside a `Box<dyn Any + Send + Sync>`, which
/// allows recovery via `value.downcast_ref::<Arc<T>>()`.
pub struct BoxedComponent {
    pub ty: TypeDescriptor,
    pub value: Box<dyn Any + Send + Sync>,
}

impl AsRef<Box<dyn Any + Send + Sync>> for BoxedComponent {
    fn as_ref(&self) -> &Box<dyn Any + Send + Sync> {
        &self.value
    }
}

impl Deref for BoxedComponent {
    type Target = Box<dyn Any + Send + Sync>;

    fn deref(&self) -> &Box<dyn Any + Send + Sync> {
        &self.value
    }
}

impl fmt::Debug for BoxedComponent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoxedComponent")
            .field("ty", &self.ty)
            .finish_non_exhaustive()
    }
}

/// One trait provider's constructed value, plus the metadata that selects it.
pub(crate) struct ProviderInstance {
    pub(crate) descriptor: ProviderDescriptor,
    pub(crate) ordinal: usize,
    pub(crate) value: BoxedComponent,
}

/// The instances and trait-providers held by one scope.
///
/// Shared by both the under-construction [`ComponentConstructionContext`] and the
/// frozen [`ScopeContainer`](crate::container::ScopeContainer); each owns one
/// `ScopeStore` and layers parent scopes on top for resolution. All lookups here
/// are **scope-local** — walking the parent chain is the caller's job. Config values
/// are *not* held here: config is an external resolver, not part of the container.
#[derive(Default)]
pub(crate) struct ScopeStore {
    pub(crate) components: HashMap<TypeId, BoxedComponent>,
    pub(crate) providers: HashMap<TypeId, Vec<ProviderInstance>>,
}

/// Recovers handle `H` from a boxed slot: downcasts to its stored representation
/// (`H::Stored`) and derives the handle. This is the single seam through which
/// every resolution converts storage back into a handle — a snapshot for `Arc<T>`,
/// a shared clone for `Dep<T>`, a plain clone for a by-value handle. `None` if the
/// box holds a different stored type.
///
/// Public so external resolvers (e.g. the config store) can recover a handle from a
/// seed they hold without re-implementing the downcast.
pub fn from_boxed<H: Injectable>(boxed: &BoxedComponent) -> Option<H> {
    boxed.value.downcast_ref::<H::Stored>().map(H::from_stored)
}

impl ScopeStore {
    /// Single concrete-or-primary-provider lookup, scope-local. `None` if absent or
    /// ambiguous.
    pub(crate) fn resolve_local<H: Injectable>(&self) -> Option<H> {
        let type_id = TypeId::of::<H::Target>();

        if let Some(component) = self.components.get(&type_id) {
            return from_boxed::<H>(component);
        }

        let chosen =
            crate::registry::selection::select_single_by(self.providers.get(&type_id)?, |entry| {
                entry.descriptor.primary
            })?;

        from_boxed::<H>(&chosen.value)
    }

    /// Qualifier-selected single provider, scope-local.
    pub(crate) fn resolve_qualified_local<H: Injectable>(&self, qualifier: &str) -> Option<H> {
        let type_id = TypeId::of::<H::Target>();
        let entry = self
            .providers
            .get(&type_id)?
            .iter()
            .find(|entry| entry.descriptor.qualifier == qualifier)?;

        from_boxed::<H>(&entry.value)
    }

    /// Resolves one exact provider descriptor from this scope.
    pub(crate) fn resolve_provider_local<H: Injectable>(
        &self,
        provider: &ProviderDescriptor,
    ) -> Option<H> {
        let entry = self
            .providers
            .get(&provider.trait_ty.type_id)?
            .iter()
            .find(|entry| {
                entry.descriptor.concrete_ty.type_id == provider.concrete_ty.type_id
                    && entry.descriptor.qualifier == provider.qualifier
            })?;

        from_boxed::<H>(&entry.value)
    }

    pub(crate) fn insert(&mut self, component: BoxedComponent) {
        let type_id = component.ty.type_id;
        self.components.insert(type_id, component);
    }

    /// Erases an already-constructed concrete component as `Arc<dyn Trait>` and
    /// records it under the trait's key, so trait-object dependencies resolve.
    pub(crate) fn register_provider(&mut self, provider: &ProviderDescriptor, ordinal: usize) {
        let concrete_id = provider.concrete_ty.type_id;

        let Some(concrete) = self.components.get(&concrete_id) else {
            return;
        };

        let erased = (provider.erase)(concrete);

        self.providers
            .entry(provider.trait_ty.type_id)
            .or_default()
            .push(ProviderInstance {
                descriptor: *provider,
                ordinal,
                value: erased,
            });

        self.providers
            .get_mut(&provider.trait_ty.type_id)
            .expect("provider entry was inserted")
            .sort_by_key(|provider| provider.ordinal);
    }

    pub(crate) fn contains(&self, type_id: TypeId) -> bool {
        self.components.contains_key(&type_id)
    }
}
