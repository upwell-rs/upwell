//! Entry plans an invocation-context accessor reaches across scopes, carried over from a base
//! generation only while every type the accessor walk read keeps its selection.

use std::sync::Arc;

use upwell_core::EntryKey;
use upwell_di::EntryPlan;

use super::*;
use crate::EntryTable;

/// A child component a root singleton reads through `Current<Arc<Accessed>>`.
struct Accessed;
/// A root singleton the entry injects, optionally depending on [`Feature`].
struct Service;
/// A root singleton a candidate turns on, reading [`Accessed`] through an accessor.
struct Feature;
/// A root singleton no entry reaches, reading [`Accessed`] through an accessor.
struct Unreached;

fn service_factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("service", || vec![DependencyDescriptor {
        optional: true,
        ..one::<Feature>("feature", "Feature")
    }])()
}

fn feature_factory() -> &'static [ComponentFactoryDescriptor] {
    unbuilt_factories!("feature", || vec![DependencyDescriptor {
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

/// The components of a generation, with [`Feature`] when `feature` is on and [`Unreached`]
/// when `unreached` is.
fn generation(
    feature: bool,
    unreached: bool,
) -> (Vec<ComponentDescriptor>, ProviderSelectionModel) {
    let mut descriptors = vec![
        descriptor::<Accessed>("accessed", "Accessed", Child::KEY, empty_factory),
        descriptor::<Service>("service", "Service", Singleton::KEY, service_factory),
    ];

    if feature {
        descriptors.push(descriptor::<Feature>(
            "feature",
            "Feature",
            Singleton::KEY,
            feature_factory,
        ));
    }

    if unreached {
        descriptors.push(descriptor::<Unreached>(
            "unreached",
            "Unreached",
            Singleton::KEY,
            feature_factory,
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

/// Plans the base generation, then the candidate with [`Feature`] on, carrying plans over.
fn turn_on_feature(unreached: bool) -> (Arc<EntryPlan>, Arc<EntryPlan>) {
    let (descriptors, selection) = generation(false, unreached);
    let (candidate_descriptors, candidate_selection) = generation(true, unreached);
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
fn turning_on_the_first_accessor_reached_through_a_singleton_replans_the_entry() {
    let (base, candidate) = turn_on_feature(false);

    assert!(!base.is_accessed());
    assert!(
        candidate.is_accessed(),
        "the feature's accessor now reads the child scope"
    );
}

#[test]
fn turning_on_an_accessor_beside_other_accessors_replans_the_entry() {
    let (base, candidate) = turn_on_feature(true);

    assert!(
        base.walked_accessors(),
        "another accessor makes planning walk"
    );
    assert!(!base.is_accessed());
    assert!(
        candidate.is_accessed(),
        "the walk read the feature's type, so its selection change replans the entry"
    );
}
