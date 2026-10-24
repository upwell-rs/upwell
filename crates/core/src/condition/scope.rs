//! The evaluation scope that answers raw config reads during one condition callback.
//!
//! Raw config reads (`resolve("<path>")`) are free functions rather than context methods,
//! so the condition evaluator installs the callback's declared raw inputs on the current
//! thread for exactly the duration of the callback. A read outside that set is recorded as
//! a [`ConditionInputRejection`] and fails the evaluation instead of becoming a hidden
//! dependency. Callbacks are synchronous, so a thread-local scope cannot leak across tasks.

use std::any::Any;
use std::cell::RefCell;
use std::fmt;
use std::sync::Arc;

/// Why a callback's input read failed the evaluation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConditionInputRejection {
    Undeclared(&'static str),
    Invalid {
        path: &'static str,
        target: &'static str,
    },
}

impl fmt::Display for ConditionInputRejection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Undeclared(path) => {
                write!(formatter, "condition read undeclared config path '{path}'")
            }
            Self::Invalid { path, target } => {
                write!(
                    formatter,
                    "config path '{path}' does not hold a valid '{target}'"
                )
            }
        }
    }
}

impl std::error::Error for ConditionInputRejection {}

/// A resolved raw config value, shared with the evaluation scope.
pub type RawConfigValue = Arc<dyn Any + Send + Sync>;

/// The raw inputs and first rejection of the callback running on this thread.
struct RawScope {
    inputs: Vec<(&'static str, Option<RawConfigValue>)>,
    rejection: Option<ConditionInputRejection>,
}

thread_local! {
    static SCOPE: RefCell<Option<RawScope>> = const { RefCell::new(None) };
}

/// Restores the enclosing scope even when the callback panics.
struct ScopeGuard {
    previous: Option<RawScope>,
}

impl Drop for ScopeGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();

        SCOPE.with(|scope| *scope.borrow_mut() = previous);
    }
}

/// Runs `evaluate` with `inputs` as the only answerable raw config paths, returning its
/// result and the first rejected read, if any.
#[doc(hidden)]
pub fn with_raw_inputs<R>(
    inputs: Vec<(&'static str, Option<RawConfigValue>)>,
    evaluate: impl FnOnce() -> R,
) -> (R, Option<ConditionInputRejection>) {
    let installed = RawScope {
        inputs,
        rejection: None,
    };
    let guard = ScopeGuard {
        previous: SCOPE.with(|scope| scope.borrow_mut().replace(installed)),
    };

    let result = evaluate();
    let rejection = SCOPE
        .with(|scope| scope.borrow_mut().take())
        .and_then(|scope| scope.rejection);

    drop(guard);

    (result, rejection)
}

/// The resolved value of a declared raw path, `None` when the path is absent.
///
/// An undeclared path is recorded as a rejection and also answers `None`.
///
/// # Panics
///
/// Panics when called outside a condition evaluation: raw config reads are only meaningful
/// against the candidate config of one evaluation.
pub fn raw_input(path: &'static str) -> Option<RawConfigValue> {
    SCOPE.with(|scope| {
        let mut scope = scope.borrow_mut();
        let scope = scope
            .as_mut()
            .expect("raw config inputs are only readable inside a condition evaluation");

        match scope.inputs.iter().find(|(declared, _)| *declared == path) {
            Some((_, value)) => value.clone(),

            None => {
                scope
                    .rejection
                    .get_or_insert(ConditionInputRejection::Undeclared(path));

                None
            }
        }
    })
}

/// Records that a declared input could not be read, failing the running evaluation.
pub fn reject_input(rejection: ConditionInputRejection) {
    SCOPE.with(|scope| {
        if let Some(scope) = scope.borrow_mut().as_mut() {
            scope.rejection.get_or_insert(rejection);
        }
    });
}
