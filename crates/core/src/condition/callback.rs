//! Trusted condition callbacks and the statically declared inputs they read.

use std::any::{Any, TypeId};
use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};

use super::{ConditionScalar, ConfigFactId, DescriptorSource, ProviderMappingId};

/// Stable identity of one typed config binding: a config type at one binding path.
#[derive(Clone, Copy)]
pub struct ConfigBindingId {
    pub type_id: TypeId,
    pub type_name: fn() -> &'static str,
    pub path: &'static str,
}

impl ConfigBindingId {
    pub const fn of<T: 'static>(path: &'static str) -> Self {
        Self {
            type_id: TypeId::of::<T>(),
            type_name: std::any::type_name::<T>,
            path,
        }
    }

    pub fn type_name(self) -> &'static str {
        (self.type_name)()
    }
}

impl fmt::Debug for ConfigBindingId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConfigBindingId")
            .field("type_name", &self.type_name())
            .field("path", &self.path)
            .finish()
    }
}

impl PartialEq for ConfigBindingId {
    fn eq(&self, other: &Self) -> bool {
        self.type_id == other.type_id && self.path == other.path
    }
}

impl Eq for ConfigBindingId {}

impl PartialOrd for ConfigBindingId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ConfigBindingId {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.path, self.type_name(), self.type_id).cmp(&(
            other.path,
            other.type_name(),
            other.type_id,
        ))
    }
}

impl Hash for ConfigBindingId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.type_id.hash(state);
        self.path.hash(state);
    }
}

/// One statically declared input of a condition callback.
///
/// Config inputs (`Fact`, `Binding`, `Raw`) invalidate the condition when they change;
/// eligibility inputs (`Component`, `ProviderMapping`) also form edges of the static
/// availability graph, so cycles through callbacks are rejected like any other.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ConditionInputId {
    /// A validated scalar config fact.
    Fact(ConfigFactId),
    /// A whole typed config binding, read as the staged typed value.
    Binding(ConfigBindingId),
    /// A raw config path that no type needs to bind, read through `resolve`.
    Raw(&'static str),
    /// Eligibility of the component with this stable ID.
    Component(&'static str),
    /// Eligibility of one exact provider mapping.
    ProviderMapping(ProviderMappingId),
}

/// The value of one declared input during a callback evaluation.
#[derive(Clone, Copy)]
pub enum ConditionInputValue<'a> {
    Fact(&'a ConditionScalar),
    /// A staged typed binding value, or a resolved raw subtree (`None` when the raw path
    /// is absent from the candidate config).
    Config(Option<&'a (dyn Any + Send + Sync)>),
    Eligible(bool),
}

impl fmt::Debug for ConditionInputValue<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fact(value) => formatter.debug_tuple("Fact").field(&value.kind()).finish(),
            Self::Config(value) => formatter
                .debug_tuple("Config")
                .field(&value.is_some())
                .finish(),
            Self::Eligible(value) => formatter.debug_tuple("Eligible").field(value).finish(),
        }
    }
}

/// Read-only access to exactly the inputs a callback declared.
#[derive(Clone, Copy)]
pub struct ConditionContext<'a> {
    inputs: &'a [(ConditionInputId, ConditionInputValue<'a>)],
}

impl<'a> ConditionContext<'a> {
    #[doc(hidden)]
    pub const fn new(inputs: &'a [(ConditionInputId, ConditionInputValue<'a>)]) -> Self {
        Self { inputs }
    }

    /// The value of `id`, or `None` when the callback did not declare it.
    pub fn input(&self, id: ConditionInputId) -> Option<ConditionInputValue<'a>> {
        self.inputs
            .iter()
            .find_map(|(candidate, value)| (*candidate == id).then_some(*value))
    }

    pub fn fact(&self, id: ConfigFactId) -> Option<&'a ConditionScalar> {
        match self.input(ConditionInputId::Fact(id))? {
            ConditionInputValue::Fact(value) => Some(value),
            _ => None,
        }
    }

    /// The staged value of config type `T` bound at `path`.
    pub fn binding<T: Any>(&self, path: &'static str) -> Option<&'a T> {
        match self.input(ConditionInputId::Binding(ConfigBindingId::of::<T>(path)))? {
            ConditionInputValue::Config(value) => value?.downcast_ref::<T>(),
            _ => None,
        }
    }

    pub fn eligible(&self, component: &'static str) -> Option<bool> {
        self.eligibility(ConditionInputId::Component(component))
    }

    pub fn provider_eligible(&self, mapping: ProviderMappingId) -> Option<bool> {
        self.eligibility(ConditionInputId::ProviderMapping(mapping))
    }

    /// Reads input `I` by type.
    pub fn get<I: ConditionInput>(&self) -> Option<I::Value<'a>> {
        I::read(*self)
    }

    fn eligibility(&self, id: ConditionInputId) -> Option<bool> {
        match self.input(id)? {
            ConditionInputValue::Eligible(value) => Some(value),
            _ => None,
        }
    }
}

/// A type that names one condition input, so generated callbacks can declare and read it
/// by type: a config type yields its staged value, a component yields its [`Eligibility`].
///
/// `#[config(path = ..)]` and the component macros implement it; a `#[when]` expression may
/// only use implementing types as field or method receivers.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be read by a component condition",
    label = "not a condition input",
    note = "a `#[when]` receiver must be a `#[config(path = ..)]` type or a component; read other config paths with `resolve(\"<path>\")`"
)]
pub trait ConditionInput: 'static {
    type Value<'a>;

    const INPUT: ConditionInputId;

    fn read<'a>(context: ConditionContext<'a>) -> Option<Self::Value<'a>>;
}

/// Unwraps a declared input read through [`ConditionInput::read`].
///
/// # Panics
///
/// Panics when the input is absent. Condition evaluation validates every declared input
/// before running a callback, so generated callbacks that declare the input never panic here.
#[doc(hidden)]
pub fn read_input<T>(value: Option<T>) -> T {
    value.expect("condition evaluation supplies every declared input")
}

/// Whether one component or provider mapping is eligible in the candidate generation.
///
/// Eligibility means the target's own condition holds; it does not construct the target
/// and says nothing about provider selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Eligibility(pub bool);

impl Eligibility {
    pub const fn eligible(self) -> bool {
        self.0
    }
}

/// Static tooling metadata for a callback written as a source expression.
#[derive(Clone, Copy, Debug)]
pub struct ConditionExpression {
    /// The expression's source text.
    pub text: &'static str,
    /// Every input read in the expression, in source order.
    pub reads: &'static [ConditionInputRead],
}

/// One input read inside a condition expression.
#[derive(Clone, Copy, Debug)]
pub struct ConditionInputRead {
    pub input: ConditionInputId,
    /// The dotted field path read from a typed config input; empty for whole-value reads,
    /// eligibility reads and raw `resolve` paths, whose declared path is the read.
    pub field: &'static str,
    pub source: DescriptorSource,
}

/// Trusted callback over statically declared config and eligibility inputs.
///
/// The context only exposes declared inputs, but Rust callbacks remain ordinary trusted
/// code. `inputs` must exhaustively declare every value that can affect the outcome;
/// incremental evaluation reuses the result while those inputs are unchanged.
/// Implementations must also be deterministic, non-blocking, panic-free, and free of
/// external side effects; the framework cannot sandbox environment, filesystem, or
/// global-state access. Tooling never executes callbacks.
#[derive(Clone, Copy)]
pub struct ConditionCallback {
    pub kind: &'static str,
    pub inputs: &'static [ConditionInputId],
    pub expression: Option<&'static ConditionExpression>,
    pub evaluate: for<'a> fn(ConditionContext<'a>) -> bool,
}

impl fmt::Debug for ConditionCallback {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConditionCallback")
            .field("kind", &self.kind)
            .field("inputs", &self.inputs)
            .field("expression", &self.expression)
            .finish_non_exhaustive()
    }
}
