use std::any::{Any, TypeId};
use std::fmt;
use std::future::Future;
use std::pin::Pin;

use upwell_core::{DependencyDescriptor, ResolverCtx, TypeDescriptor, UpwellDescriptor};

use crate::{HookKind, HookRole, HookRouting, Result};

/// The erased call shape every hook is compiled to: resolve the receiver (if the method
/// takes `&self`) from the resolver context, extract the kind's params from the erased
/// context, run the method, and box its output. A participant's boxed value is always the
/// kind's `Output`; an observer's is discarded.
///
/// The call takes `&dyn ResolverCtx` (not the container itself), so the hook layer stays
/// below the DI engine. The macro-generated body resolves the receiver through the
/// component source it fetches from the context.
pub type HookCall =
    for<'a> fn(
        &'a (dyn ResolverCtx + Send + Sync),
        &'a (dyn Any + Send + Sync),
    ) -> Pin<Box<dyn Future<Output = Result<Box<dyn Any + Send>>> + Send + 'a>>;

/// Static metadata for one hook, registered into its type's registration set.
///
/// `#[non_exhaustive]`: construct with [`HookDescriptor::new`], never a struct literal, so fields
/// (like [`ordinal`](Self::ordinal)) can be added without breaking downstream construction. The
/// macros and any hand-written descriptor site call `new`.
#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct HookDescriptor {
    /// Source-position ordinal (the hook method's line), used to run a type's hooks in a stable
    /// source order regardless of registration backend. `linkme` preserves source order via link
    /// order; `inventory`'s linked list does not, so the accessor sorts by this.
    pub ordinal: u32,
    /// The component/service the hook is defined on.
    pub component_ty: TypeDescriptor,
    /// The hook kind's [`NAME`](crate::HookKind::NAME), for diagnostics.
    pub kind: &'static str,
    /// `TypeId::of::<Kind>()`, used to select hooks of a given kind at runtime.
    pub kind_ty: fn() -> TypeId,
    /// The hook's parameter edges (kind inputs), reported at runtime for validation and
    /// event routing.
    pub dependencies: fn() -> Vec<DependencyDescriptor>,
    /// The erased call.
    pub call: HookCall,
    /// The declared role. A hook of an
    /// [`ObserversOnly`](crate::HookParticipation::ObserversOnly) kind is an observer
    /// regardless.
    pub role: HookRole,
    /// Whether the hook runs only when its firing site selects it.
    pub routing: HookRouting,
}

impl HookDescriptor {
    /// Builds a targeted participant hook descriptor. The canonical constructor — required
    /// because the type is `#[non_exhaustive]` — so macro-generated and hand-written
    /// registration sites survive future field additions. `const`, so it is usable in the
    /// `static`s the `linkme` backend emits.
    pub const fn new(
        ordinal: u32,
        component_ty: TypeDescriptor,
        kind: &'static str,
        kind_ty: fn() -> TypeId,
        dependencies: fn() -> Vec<DependencyDescriptor>,
        call: HookCall,
    ) -> Self {
        Self {
            ordinal,
            component_ty,
            kind,
            kind_ty,
            dependencies,
            call,
            role: HookRole::Participant,
            routing: HookRouting::Targeted,
        }
    }

    /// Builds a targeted participant hook of kind `K` on `component_ty` that takes no
    /// kind inputs; add them with [`with_dependencies`](Self::with_dependencies).
    pub const fn of<K: HookKind>(
        ordinal: u32,
        component_ty: TypeDescriptor,
        call: HookCall,
    ) -> Self {
        Self::new(
            ordinal,
            component_ty,
            K::NAME,
            kind_type::<K>,
            no_dependencies,
            call,
        )
    }

    /// Declares the hook's parameter edges.
    pub const fn with_dependencies(
        mut self,
        dependencies: fn() -> Vec<DependencyDescriptor>,
    ) -> Self {
        self.dependencies = dependencies;

        self
    }

    /// Declares the hook's role.
    pub const fn with_role(mut self, role: HookRole) -> Self {
        self.role = role;

        self
    }

    /// Declares the hook's routing.
    pub const fn with_routing(mut self, routing: HookRouting) -> Self {
        self.routing = routing;

        self
    }
}

impl UpwellDescriptor for HookDescriptor {}

impl fmt::Debug for HookDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HookDescriptor")
            .field("component_ty", &self.component_ty)
            .field("kind", &self.kind)
            .field("role", &self.role)
            .field("routing", &self.routing)
            .field("dependencies", &(self.dependencies)())
            .finish_non_exhaustive()
    }
}

/// A component type's own hooks.
///
/// Implemented for each `#[component]`/`#[service]` by the macro to return that type's
/// `{Type}Hooks` distributed slice — the slice every `#[hook]` method appends to. The owning
/// `ComponentDescriptor` stores this as a fn pointer, so the registry reaches a type's hooks
/// without holding its type.
pub trait ComponentHooks {
    /// Every hook contributed to this type.
    fn hooks() -> &'static [HookDescriptor];
}

fn kind_type<K: 'static>() -> TypeId {
    TypeId::of::<K>()
}

fn no_dependencies() -> Vec<DependencyDescriptor> {
    Vec::new()
}

/// The empty hooks slice for a type that declares none — the default carried by a
/// `ComponentDescriptor`.
pub fn no_hooks() -> &'static [HookDescriptor] {
    &[]
}
