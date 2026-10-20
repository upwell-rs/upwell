//! Shape recovery by tracing a type's serde `Deserialize` impl.
//!
//! A [`Tracer`](tracer::Tracer) answers every `deserialize_*` request with a plausible value while
//! recording what was requested, so a type's own impl reveals its fields, scalars, and collections
//! — including types that never passed through `#[config]`.
//!
//! A type's validation can reject a traced value (an `IpAddr` parsed from a string, a duration
//! written `"5s"`). Tracing then re-runs with the next candidate value for that field, and a field
//! that rejects every candidate is skipped with the shape it was recorded with, so one strict field
//! never hides the fields declared after it.

use std::cell::RefCell;
use std::collections::HashMap;

use serde::de::DeserializeOwned;

use super::{TracedField, ValueSchema};

mod access;
mod tracer;

/// Bounds re-runs so a type that keeps rejecting values still terminates.
const MAX_ATTEMPTS: usize = 64;

/// The traced shape of `T`.
///
/// Best effort: a self-describing request (`deserialize_any`, as untagged and internally tagged
/// enums issue) records [`ValueSchema::Any`] at that position, keeping everything else.
pub fn trace<T: DeserializeOwned>() -> ValueSchema {
    let state = TraceState::default();
    let mut schema = ValueSchema::Any;

    for _ in 0..MAX_ATTEMPTS {
        schema = ValueSchema::Any;
        state.begin();

        let traced = T::deserialize(tracer::Tracer::new(&mut schema, 0, &state));

        if traced.is_ok() || !state.advance() {
            break;
        }
    }

    schema
}

/// A field path rejected by the type's own validation during one run.
struct Rejection {
    path: String,
    schema: ValueSchema,
    candidates: usize,
}

/// What tracing has learned across runs: the candidate chosen per field path, and the fields
/// skipped because they rejected every candidate.
#[derive(Default)]
struct TraceState {
    choices: RefCell<HashMap<String, usize>>,
    skipped: RefCell<HashMap<String, ValueSchema>>,
    path: RefCell<Vec<String>>,
    rejection: RefCell<Option<Rejection>>,
}

impl TraceState {
    fn begin(&self) {
        self.path.borrow_mut().clear();
        *self.rejection.borrow_mut() = None;
    }

    /// Learns from the last run's rejection, returning whether another run can get further.
    fn advance(&self) -> bool {
        let Some(rejection) = self.rejection.borrow_mut().take() else {
            return false;
        };
        let mut choices = self.choices.borrow_mut();
        let choice = choices.entry(rejection.path.clone()).or_default();

        if *choice + 1 < rejection.candidates {
            *choice += 1;
        } else {
            self.skipped
                .borrow_mut()
                .insert(rejection.path, rejection.schema);
        }

        true
    }

    fn current_path(&self) -> String {
        self.path.borrow().join("/")
    }

    fn enter(&self, segment: &str) {
        self.path.borrow_mut().push(segment.to_string());
    }

    fn leave(&self) {
        self.path.borrow_mut().pop();
    }

    /// The candidate index to offer at the current path.
    fn choice(&self) -> usize {
        self.choices
            .borrow()
            .get(&self.current_path())
            .copied()
            .unwrap_or_default()
    }

    /// The recorded shape of a skipped field at `path`, if it was skipped.
    fn skipped(&self, path: &str) -> Option<ValueSchema> {
        self.skipped.borrow().get(path).cloned()
    }

    /// Records that the value offered at the current path was rejected. The innermost rejection
    /// wins: an enclosing field only reports when nothing inside it did.
    fn reject(&self, schema: ValueSchema, candidates: usize) {
        let mut rejection = self.rejection.borrow_mut();

        if rejection.is_none() {
            *rejection = Some(Rejection {
                path: self.current_path(),
                schema,
                candidates,
            });
        }
    }
}
