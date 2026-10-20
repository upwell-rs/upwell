//! `const` builders for [`ConditionDescriptor`] nodes.
//!
//! Each builder records its call site as the node's [`DescriptorSource`], so a
//! hand-declared condition reports the same diagnostics as a macro-generated one.
//! Builders are `const` and compose into the `static` descriptors components carry:
//!
//! ```
//! use upwell_core::{ConditionDescriptor, ConfigFactId};
//!
//! const MODE: ConfigFactId = ConfigFactId::new("AuthConfig", "auth", "mode");
//!
//! static CUSTOM: ConditionDescriptor = ConditionDescriptor::config_equals("custom", MODE, "custom");
//! static DEFAULT: ConditionDescriptor = ConditionDescriptor::not("default", &CUSTOM);
//! ```

use super::{
    ConditionCallback, ConditionDescriptor, ConditionPredicate, ConditionScalarLiteral,
    ConfigFactId, DescriptorSource, ProviderMappingId,
};

impl ConditionDescriptor {
    /// A node with `predicate`, sourced at the caller.
    #[track_caller]
    pub const fn new(id: &'static str, predicate: ConditionPredicate) -> Self {
        Self {
            id,
            source: DescriptorSource::caller(),
            predicate,
        }
    }

    /// Holds when the boolean config fact is `true`.
    #[track_caller]
    pub const fn config_bool(id: &'static str, fact: ConfigFactId) -> Self {
        Self::new(id, ConditionPredicate::ConfigBool(fact))
    }

    /// Holds when the string config fact equals `expected`.
    #[track_caller]
    pub const fn config_equals(
        id: &'static str,
        fact: ConfigFactId,
        expected: &'static str,
    ) -> Self {
        Self::config_matches(id, fact, ConditionScalarLiteral::String(expected))
    }

    /// Holds when the config fact equals `expected`, of any scalar kind.
    #[track_caller]
    pub const fn config_matches(
        id: &'static str,
        fact: ConfigFactId,
        expected: ConditionScalarLiteral,
    ) -> Self {
        Self::new(id, ConditionPredicate::ConfigEquals { fact, expected })
    }

    /// Holds when the component with stable ID `component` is eligible.
    #[track_caller]
    pub const fn component_eligible(id: &'static str, component: &'static str) -> Self {
        Self::new(id, ConditionPredicate::ComponentEligible(component))
    }

    /// Holds when the provider mapping is eligible.
    #[track_caller]
    pub const fn provider_eligible(id: &'static str, mapping: ProviderMappingId) -> Self {
        Self::new(id, ConditionPredicate::ProviderMappingEligible(mapping))
    }

    /// Holds when the trusted `callback` returns `true` for its declared inputs.
    #[track_caller]
    pub const fn callback(id: &'static str, callback: &'static ConditionCallback) -> Self {
        Self::new(id, ConditionPredicate::Callback(callback))
    }

    /// Holds when every node in `all` holds.
    #[track_caller]
    pub const fn all(id: &'static str, all: &'static [Self]) -> Self {
        Self::new(id, ConditionPredicate::All(all))
    }

    /// Holds when at least one node in `any` holds.
    #[track_caller]
    pub const fn any(id: &'static str, any: &'static [Self]) -> Self {
        Self::new(id, ConditionPredicate::Any(any))
    }

    /// Holds when `negated` does not.
    #[track_caller]
    pub const fn not(id: &'static str, negated: &'static Self) -> Self {
        Self::new(id, ConditionPredicate::Not(negated))
    }
}
