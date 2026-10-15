use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Captures the call site's source location for descriptor diagnostics.
#[macro_export]
macro_rules! descriptor_source {
    () => {
        $crate::DescriptorSource::new(file!(), line!(), column!())
    };
}

/// Static source location for descriptor diagnostics.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DescriptorSource {
    pub file: &'static str,
    pub line: u32,
    pub column: u32,
}

impl Default for DescriptorSource {
    fn default() -> Self {
        Self::UNKNOWN
    }
}

impl DescriptorSource {
    pub const UNKNOWN: Self = Self {
        file: "<unknown>",
        line: 0,
        column: 0,
    };

    pub const fn new(file: &'static str, line: u32, column: u32) -> Self {
        Self { file, line, column }
    }
}

/// Stable identity of one typed scalar config fact.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ConfigFactId {
    pub config_type: &'static str,
    pub binding_path: &'static str,
    pub property_path: &'static str,
}

impl ConfigFactId {
    pub const fn new(
        config_type: &'static str,
        binding_path: &'static str,
        property_path: &'static str,
    ) -> Self {
        Self {
            config_type,
            binding_path,
            property_path,
        }
    }
}

/// Stable structural identity of one concrete-component to trait mapping.
#[derive(Clone, Copy)]
pub struct ProviderMappingId {
    pub component: &'static str,
    pub trait_type: fn() -> &'static str,
    pub qualifier: &'static str,
}

impl ProviderMappingId {
    pub const fn of<P: ?Sized + 'static>(component: &'static str, qualifier: &'static str) -> Self {
        Self {
            component,
            trait_type: std::any::type_name::<P>,
            qualifier,
        }
    }

    pub fn trait_name(self) -> &'static str {
        (self.trait_type)()
    }
}

impl fmt::Debug for ProviderMappingId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderMappingId")
            .field("component", &self.component)
            .field("trait_type", &(self.trait_type)())
            .field("qualifier", &self.qualifier)
            .finish()
    }
}

impl fmt::Display for ProviderMappingId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let trait_type = (self.trait_type)();

        write!(
            formatter,
            "{}#{}{}#{}{}#{}",
            self.component.len(),
            self.component,
            trait_type.len(),
            trait_type,
            self.qualifier.len(),
            self.qualifier
        )
    }
}

impl PartialEq for ProviderMappingId {
    fn eq(&self, other: &Self) -> bool {
        self.component == other.component
            && (self.trait_type)() == (other.trait_type)()
            && self.qualifier == other.qualifier
    }
}

impl Eq for ProviderMappingId {}

impl PartialOrd for ProviderMappingId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ProviderMappingId {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.component, (self.trait_type)(), self.qualifier).cmp(&(
            other.component,
            (other.trait_type)(),
            other.qualifier,
        ))
    }
}

impl Hash for ProviderMappingId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.component.hash(state);
        (self.trait_type)().hash(state);
        self.qualifier.hash(state);
    }
}

/// Scalar kinds supported by the first conditional-component contract.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ConditionScalarKind {
    Bool,
    Integer,
    String,
    EnumToken,
}

/// A static scalar literal used by a condition.
#[derive(Clone, Copy)]
pub enum ConditionScalarLiteral {
    Bool(bool),
    Integer(i128),
    String(&'static str),
    EnumToken(&'static str),
}

impl ConditionScalarLiteral {
    pub const fn kind(self) -> ConditionScalarKind {
        match self {
            Self::Bool(_) => ConditionScalarKind::Bool,
            Self::Integer(_) => ConditionScalarKind::Integer,
            Self::String(_) => ConditionScalarKind::String,
            Self::EnumToken(_) => ConditionScalarKind::EnumToken,
        }
    }

    pub fn matches(self, value: &ConditionScalar) -> bool {
        match (self, value) {
            (Self::Bool(expected), ConditionScalar::Bool(actual)) => expected == *actual,
            (Self::Integer(expected), ConditionScalar::Integer(actual)) => expected == *actual,
            (Self::String(expected), ConditionScalar::String(actual)) => {
                expected == actual.as_ref()
            }
            (Self::EnumToken(expected), ConditionScalar::EnumToken(actual)) => {
                expected == actual.as_ref()
            }
            _ => false,
        }
    }
}

impl fmt::Debug for ConditionScalarLiteral {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ConditionScalarLiteral")
            .field(&self.kind())
            .finish()
    }
}

/// An owned scalar supplied to condition evaluation.
#[derive(Clone, Eq, PartialEq)]
pub enum ConditionScalar {
    Bool(bool),
    Integer(i128),
    String(Arc<str>),
    EnumToken(Arc<str>),
}

impl ConditionScalar {
    pub fn string(value: impl Into<Arc<str>>) -> Self {
        Self::String(value.into())
    }

    pub fn enum_token(value: impl Into<Arc<str>>) -> Self {
        Self::EnumToken(value.into())
    }

    pub const fn kind(&self) -> ConditionScalarKind {
        match self {
            Self::Bool(_) => ConditionScalarKind::Bool,
            Self::Integer(_) => ConditionScalarKind::Integer,
            Self::String(_) => ConditionScalarKind::String,
            Self::EnumToken(_) => ConditionScalarKind::EnumToken,
        }
    }

    pub const fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }
}

/// Read-only access to the statically declared config inputs of a callback predicate.
#[derive(Clone, Copy)]
pub struct ConfigConditionContext<'a> {
    inputs: &'a [(ConfigFactId, &'a ConditionScalar)],
}

impl<'a> ConfigConditionContext<'a> {
    #[doc(hidden)]
    pub const fn new(inputs: &'a [(ConfigFactId, &'a ConditionScalar)]) -> Self {
        Self { inputs }
    }

    pub fn get(&self, id: ConfigFactId) -> Option<&'a ConditionScalar> {
        self.inputs
            .iter()
            .find_map(|(candidate, value)| (*candidate == id).then_some(*value))
    }
}

/// One statically declared availability input of a callback predicate.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AvailabilityTarget {
    Component(&'static str),
    ProviderMapping(ProviderMappingId),
}

/// Read-only access to the statically declared availability inputs of a callback predicate.
#[derive(Clone, Copy)]
pub struct AvailabilityConditionContext<'a> {
    inputs: &'a [(AvailabilityTarget, bool)],
}

impl<'a> AvailabilityConditionContext<'a> {
    #[doc(hidden)]
    pub const fn new(inputs: &'a [(AvailabilityTarget, bool)]) -> Self {
        Self { inputs }
    }

    pub fn eligible(&self, target: AvailabilityTarget) -> Option<bool> {
        self.inputs
            .iter()
            .find_map(|(candidate, value)| (*candidate == target).then_some(*value))
    }
}

/// Trusted callback over statically declared, validated scalar config facts.
///
/// The context prevents undeclared framework lookups, but Rust callbacks remain ordinary trusted
/// code. `inputs` must exhaustively declare every value that can affect the outcome; incremental
/// evaluation reuses the result while those inputs are unchanged. Implementations must also be
/// deterministic, non-blocking, panic-free, and free of external side effects; the framework
/// cannot sandbox environment, filesystem, or global-state access.
#[derive(Clone, Copy)]
pub struct ConfigConditionCallback {
    pub kind: &'static str,
    pub inputs: &'static [ConfigFactId],
    pub evaluate: for<'a> fn(ConfigConditionContext<'a>) -> bool,
}

impl fmt::Debug for ConfigConditionCallback {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConfigConditionCallback")
            .field("kind", &self.kind)
            .field("inputs", &self.inputs)
            .finish_non_exhaustive()
    }
}

/// Trusted callback over statically declared component and provider-mapping eligibility.
///
/// `inputs` must exhaustively declare every eligibility value that can affect the outcome. The
/// same purity and failure contract as [`ConfigConditionCallback`] applies.
#[derive(Clone, Copy)]
pub struct AvailabilityConditionCallback {
    pub kind: &'static str,
    pub inputs: &'static [AvailabilityTarget],
    pub evaluate: for<'a> fn(AvailabilityConditionContext<'a>) -> bool,
}

impl fmt::Debug for AvailabilityConditionCallback {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AvailabilityConditionCallback")
            .field("kind", &self.kind)
            .field("inputs", &self.inputs)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for ConditionScalar {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ConditionScalar")
            .field(&self.kind())
            .finish()
    }
}

/// Declares one config fact accepted by a condition catalog.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConfigFactDescriptor {
    pub id: ConfigFactId,
    pub kind: ConditionScalarKind,
    pub source: DescriptorSource,
}

/// One closed, inspectable condition predicate.
#[derive(Clone, Copy)]
pub enum ConditionPredicate {
    ConfigBool(ConfigFactId),
    ConfigEquals {
        fact: ConfigFactId,
        expected: ConditionScalarLiteral,
    },
    ComponentEligible(&'static str),
    ProviderMappingEligible(ProviderMappingId),
    ConfigCallback(&'static ConfigConditionCallback),
    AvailabilityCallback(&'static AvailabilityConditionCallback),
    All(&'static [ConditionDescriptor]),
    Any(&'static [ConditionDescriptor]),
    Not(&'static ConditionDescriptor),
}

impl ConditionPredicate {
    pub const fn kind(self) -> ConditionPredicateKind {
        match self {
            Self::ConfigBool(_) => ConditionPredicateKind::ConfigBool,
            Self::ConfigEquals { .. } => ConditionPredicateKind::ConfigEquals,
            Self::ComponentEligible(_) => ConditionPredicateKind::ComponentEligible,
            Self::ProviderMappingEligible(_) => ConditionPredicateKind::ProviderMappingEligible,
            Self::ConfigCallback(_) => ConditionPredicateKind::ConfigCallback,
            Self::AvailabilityCallback(_) => ConditionPredicateKind::AvailabilityCallback,
            Self::All(_) => ConditionPredicateKind::All,
            Self::Any(_) => ConditionPredicateKind::Any,
            Self::Not(_) => ConditionPredicateKind::Not,
        }
    }
}

impl fmt::Debug for ConditionPredicate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConditionPredicate")
            .field("kind", &self.kind())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ConditionPredicateKind {
    ConfigBool,
    ConfigEquals,
    ComponentEligible,
    ProviderMappingEligible,
    ConfigCallback,
    AvailabilityCallback,
    All,
    Any,
    Not,
}

/// One owner-scoped node in a component's static condition expression.
#[derive(Clone, Copy, Debug)]
pub struct ConditionDescriptor {
    pub id: &'static str,
    pub source: DescriptorSource,
    pub predicate: ConditionPredicate,
}
