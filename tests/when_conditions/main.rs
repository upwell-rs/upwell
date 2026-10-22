//! End-to-end acceptance of the `#[when]` condition attribute (#297) through the `upwell`
//! facade: the `Authenticator` example's conditions are written with attributes only, follow
//! config changes in both directions through the transactional reload, and lower to
//! inspectable descriptors whose ids do not depend on declaration order.
#![allow(dead_code)]

mod app;
mod descriptors;
mod registration;
mod selection;
#[cfg(feature = "tooling")]
mod tooling;
