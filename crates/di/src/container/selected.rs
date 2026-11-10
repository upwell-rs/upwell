//! Reading built components and providers from the sources a scope's layout selected for
//! them, without selecting providers per call.

use std::any::TypeId;
use std::collections::HashMap;

use upwell_core::Cardinality;

use super::ScopeContainer;
use crate::descriptors::Injectable;
use crate::descriptors::component::from_boxed;
use crate::layout::{DepRef, ProvidedRef, SlotLayout};

impl ScopeContainer {
    /// The component of type `H::Target` built in this scope or the scopes it opens under.
    /// A type nothing provides as a trait is found by its concrete type alone; one that is
    /// provided as a trait is read through [`selected_built`](Self::selected_built).
    pub(crate) fn built_component<H: Injectable>(&self) -> Option<H> {
        let type_id = TypeId::of::<H::Target>();
        let mut current = self;

        if !self
            .registry
            .selection
            .providers_for_trait(type_id)
            .is_empty()
        {
            return self.selected_built::<H>(type_id, None);
        }

        loop {
            if let Some(component) = current.store.component(type_id) {
                return from_boxed::<H>(component);
            }

            current = current.parent.as_deref()?;
        }
    }

    /// The built provider of trait `target`, selected by `qualifier`, that runtime selection
    /// picks from this scope, read from the source this scope's layout selected for it.
    ///
    /// `None` sends the caller down runtime selection: when this scope was not opened from a
    /// layout, the layout selected no planned non-transient source, a scope on this chain
    /// shadows the selected provider, the source is not readable on this chain (an
    /// enclosing scope opened without that layout), or the instance is not built yet.
    ///
    /// Like planned edges, this relies on a provider's instance living only in the store of
    /// its component's scope, so the source holds the instance selection would read, and on
    /// a scope being opened with the registry of the selection its layout was planned from.
    pub(crate) fn selected_built<H: Injectable>(
        &self,
        target: TypeId,
        qualifier: Option<&str>,
    ) -> Option<H> {
        let layout = self.store.slots()?.layout();
        let selected = layout.selected(target, qualifier)?;

        if selected
            .shadowed_by
            .iter()
            .any(|scope| self.can_access(*scope))
        {
            return None;
        }

        self.read_selected::<H>(layout, &selected.source)
    }

    /// Every built provider of trait `H::Target` a collection dependency gathers from this
    /// scope, in the order it gathers them, read from the sources this scope's layout selected
    /// for them. The only allocation is the returned `Vec`.
    ///
    /// `None` sends the caller down runtime selection, as for
    /// [`selected_built`](Self::selected_built): this scope was not opened from a layout, the
    /// layout selected no planned non-transient source for every provider, a scope on this
    /// chain the layout could not reach holds a provider of the trait, or one of the sources
    /// is not readable or not built here.
    pub(crate) fn built_collection<H: Injectable>(&self) -> Option<Vec<H>> {
        let (layout, provided) =
            self.selected_many(TypeId::of::<H::Target>(), Cardinality::Collection)?;
        let mut all = Vec::with_capacity(provided.len());

        for entry in provided {
            all.push(self.read_selected::<H>(layout, &entry.source)?);
        }

        Some(all)
    }

    /// The built provider winning each qualifier of trait `H::Target` that a keyed dependency
    /// gathers from this scope, read as [`built_collection`](Self::built_collection) reads a
    /// collection. It allocates the map and its keys.
    pub(crate) fn built_keyed<H: Injectable>(&self) -> Option<HashMap<String, H>> {
        let (layout, provided) =
            self.selected_many(TypeId::of::<H::Target>(), Cardinality::Keyed)?;
        let mut keyed = HashMap::with_capacity(provided.len());

        for entry in provided {
            let handle = self.read_selected::<H>(layout, &entry.source)?;

            keyed.insert(entry.qualifier.to_string(), handle);
        }

        Some(keyed)
    }

    /// This scope's layout and the providers it selected for a `cardinality` dependency on
    /// trait `target`, unless a scope on this chain the layout could not reach holds one.
    fn selected_many(
        &self,
        target: TypeId,
        cardinality: Cardinality,
    ) -> Option<(&SlotLayout, &[ProvidedRef])> {
        let layout = self.store.slots()?.layout();
        let selected = layout.selected_many(target, cardinality)?;

        if selected
            .shadowed_by
            .iter()
            .any(|scope| self.can_access(*scope))
        {
            return None;
        }

        Some((layout, &selected.providers))
    }

    /// The handle a source this scope's `layout` selected resolves to on this chain.
    fn read_selected<H: Injectable>(&self, layout: &SlotLayout, source: &DepRef) -> Option<H> {
        match source {
            DepRef::Local(at) => self.planned::<H>(layout, *at),
            source => self.read_enclosing::<H>(source),
        }
    }
}
