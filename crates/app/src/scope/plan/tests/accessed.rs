//! Entry plans of a scope an invocation-context accessor reads, across reloads that turn
//! accessors on and off.

use std::sync::Arc;

use upwell_core::EntryKey;
use upwell_di::EntryPlan;

use super::*;
use crate::EntryTable;

/// A child component root singletons read through `Current<Arc<Accessed>>`.
struct Accessed;
/// A root singleton the entry injects, optionally depending on [`Feature`].
struct Service;
/// A root singleton reached through [`Service`], reading [`Accessed`] through an accessor.
struct Feature;
/// A root singleton no entry reaches, reading [`Accessed`] through an accessor.
struct Unreached;

fn service_factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("service", || vec![DependencyDescriptor {
        optional: true,
        ..one::<Feature>("feature", "Feature")
    }])()
}

fn accessor_factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("accessor", || vec![DependencyDescriptor {
        resolution: ResolutionMode::Current,
        ..one::<Accessed>("accessed", "Accessed")
    }])()
}

fn on_service() -> Vec<DependencyDescriptor> {
    vec![one::<Service>("service", "Service")]
}

fn entries() -> Vec<EntryDescriptor> {
    vec![EntryDescriptor {
        key: EntryKey::new("test/service").expect("valid test entry key"),
        scope: Child::KEY,
        targets: on_service,
        owner: None,
        source: None,
    }]
}

/// Which accessor-bearing singletons a generation enables.
#[derive(Clone, Copy)]
struct Enabled {
    feature: bool,
    unreached: bool,
}

const NONE: Enabled = Enabled {
    feature: false,
    unreached: false,
};
const FEATURE: Enabled = Enabled {
    feature: true,
    unreached: false,
};
const UNREACHED: Enabled = Enabled {
    feature: false,
    unreached: true,
};

fn generation(enabled: Enabled) -> (Vec<ComponentDescriptor>, ProviderSelectionModel) {
    let mut descriptors = vec![
        descriptor::<Accessed>("accessed", "Accessed", Child::KEY, empty_factory),
        descriptor::<Service>("service", "Service", Singleton::KEY, service_factory),
    ];

    if enabled.feature {
        descriptors.push(descriptor::<Feature>(
            "feature",
            "Feature",
            Singleton::KEY,
            accessor_factory,
        ));
    }

    if enabled.unreached {
        descriptors.push(descriptor::<Unreached>(
            "unreached",
            "Unreached",
            Singleton::KEY,
            accessor_factory,
        ));
    }

    let selection = selection_with(&descriptors, Vec::new());

    (descriptors, selection)
}

fn plan(
    descriptors: &[ComponentDescriptor],
    selection: &ProviderSelectionModel,
    base: Option<PlanBase<'_>>,
) -> ScopePlan {
    let topology = topology();
    let planning = EntryPlanning {
        selection,
        topology: &topology,
        unplanned: UnplannedConstruction::Deny,
        parallelism: Parallelism::available(),
    };

    ScopePlan::partition_from(descriptors, &entries(), &planning, base)
        .expect("the test generation plans")
}

fn service_plan(plan: &ScopePlan) -> &Arc<EntryPlan> {
    let table = EntryTable::new(&entries(), &topology()).expect("the test entries validate");
    let id = table
        .id(&EntryKey::new("test/service").expect("valid test entry key"))
        .expect("the entry is registered");

    plan.layouts.shared_entry(id).expect("the entry has a plan")
}

/// The service entry's plan before and after a reload from `before` to `after`, carrying
/// plans over where they hold.
fn reload(before: Enabled, after: Enabled) -> (Arc<EntryPlan>, Arc<EntryPlan>) {
    let (descriptors, selection) = generation(before);
    let (candidate_descriptors, candidate_selection) = generation(after);
    let base = plan(&descriptors, &selection, None);

    let candidate = plan(
        &candidate_descriptors,
        &candidate_selection,
        Some(PlanBase {
            layouts: &base.layouts,
            selection: &selection,
        }),
    );

    (
        Arc::clone(service_plan(&base)),
        Arc::clone(service_plan(&candidate)),
    )
}

#[test]
fn turning_an_accessor_reached_through_a_singleton_on_and_off_replans_the_entry() {
    let (off, on) = reload(NONE, FEATURE);
    let (was_on, back_off) = reload(FEATURE, NONE);

    assert!(!off.is_accessed() && on.is_accessed());
    assert!(was_on.is_accessed() && !back_off.is_accessed());
}

#[test]
fn turning_the_last_unreached_accessor_on_and_off_replans_the_entry() {
    let (off, on) = reload(NONE, UNREACHED);
    let (was_on, back_off) = reload(UNREACHED, NONE);

    assert!(
        !off.is_accessed() && on.is_accessed(),
        "an accessor no entry reaches still makes the scope accessed"
    );
    assert!(
        was_on.is_accessed() && !back_off.is_accessed(),
        "disabling the last accessor plans the scope's entries again"
    );
}

#[test]
fn a_reload_without_accessors_carries_the_plan_over_unchanged() {
    let (base, candidate) = reload(NONE, NONE);

    assert!(Arc::ptr_eq(&base, &candidate));
    assert!(!candidate.is_accessed());
}
