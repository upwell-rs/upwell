//! Acceptance of the transition-recipe attributes (#298) through the `upwell` facade:
//! `#[replace]`, `#[transition]` and `#[hot_update]` generate the same descriptors as the
//! `factories!` builder form, and each one drives its strategy when a reload switches the
//! `Mode` provider and relabels the banner, in both directions.
#![allow(dead_code)]

mod app;
mod descriptors;
mod reload;
