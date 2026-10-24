//! DI-trait impls for framework singletons defined in lower crates.
//!
//! [`HookManager`] lives in `upwell-hooks` (below di), so its `Component`/`Injectable`
//! impls — which name di's own traits — must live here, where di can see both. This is
//! the orphan rule working in our favour: the trait is local to di.

use upwell_hooks::{HOOK_MANAGER_ID, HOOK_MANAGER_NAME, HookManager};

use crate::descriptors::{Component, Injectable};

impl Component for HookManager {
    type Handle = HookManager;

    const ID: &'static str = HOOK_MANAGER_ID;
    const NAME: &'static str = HOOK_MANAGER_NAME;

    fn into_handle(self) -> Self::Handle {
        self
    }
}

impl Injectable for HookManager {
    type Target = HookManager;
    type Stored = Self;

    fn into_stored(self) -> Self {
        self
    }

    fn from_stored(stored: &Self) -> Self {
        stored.clone()
    }
}

/// Under `di-check`, the hook manager is framework-seeded, so it is always provided.
#[cfg(feature = "di-check")]
impl crate::descriptors::Provided<HookManager> for crate::descriptors::Wiring {}
