//! The slots of a scope that invocation-context accessors read.
//!
//! A `Current<T>` or `Scoped<S, T>` edge is resolved from the active invocation when it is
//! read, never with its consumer, and its consumer may live in any scope: a singleton built
//! long before any instance of this one opens, or a component of an enclosing or nested
//! scope. The layout therefore marks, once per generation, every slot of its scope that any
//! such edge in the registry resolves to, with the slots of this scope those depend on. Every
//! entry of the scope declares them without building them, every opening of the scope captures
//! the seeds they read, and a scope with any such slot opens at once instead of waiting for
//! its first resolution. Construction itself stays lazy, at `get`.
//!
//! The set is per scope, not per entry: in an application where an accessor reads a scope,
//! every entry of that scope opens it at once and captures the seeds the accessed values read,
//! whether or not that entry reaches the accessor. In exchange it holds wherever and however
//! the accessor's consumer is reached (an entry target, an eager or already built component,
//! middleware, a nested scope), and it changes with the layout, so a reload that changes it
//! plans the scope's entries again. The edge of a `Scoped<S, T>` does not carry `S`, so it is
//! marked in every scope that provides `T`.

use upwell_core::{ResolutionMode, ScopeId};

use super::SlotLayout;
use super::entry_plan::EagerSelection;
use crate::registry::ProviderSelectionModel;
use crate::registry::scopes::scope_allows_with;

impl SlotLayout {
    /// The accessed set: the slots of this scope every invocation-context edge of every
    /// component in `selection` resolves to, with their dependencies here. `can_reach` is the
    /// scope reachability `selection` was validated with.
    pub(super) fn accessed_closure(
        &self,
        selection: &ProviderSelectionModel,
        can_reach: &impl Fn(ScopeId, ScopeId) -> bool,
    ) -> Box<[bool]> {
        let can_access = |consumer, dependency| scope_allows_with(consumer, dependency, can_reach);
        let selector = EagerSelection {
            selection,
            can_access: &can_access,
        };
        let mut accessed = vec![false; self.slots.len()].into_boxed_slice();
        let edges = selection
            .components()
            .filter_map(|component| component.effective_factory().ok().flatten())
            .flat_map(|factory| (factory.dependencies)())
            .filter(|dependency| dependency.resolution == ResolutionMode::Current);

        for dependency in edges {
            self.for_each_selected_slot(
                &selector,
                dependency.ty.type_id,
                dependency.qualifier,
                dependency.cardinality,
                |slot| self.include(&selector, slot, &mut accessed),
            );
        }

        accessed
    }
}
