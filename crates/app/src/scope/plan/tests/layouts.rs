//! Entry plans at scale: one plan per signature, the same plans on any number of threads,
//! and plans carried over from a base generation only while they still hold.

use std::sync::Arc;

use upwell_core::EntryKey;
use upwell_di::EntryPlan;

use super::*;
use crate::EntryTable;
use crate::entry::EntryId;

/// A child component entries target directly.
struct PlannedA;
/// A second child component entries target directly.
struct PlannedB;
/// A child component added by a candidate, changing the child layout.
struct PlannedLate;
/// The child provider of [`Planned`].
struct ChildPlanned;
/// A root provider of [`Planned`] a candidate turns on.
struct RootPlanned;

trait Planned: Send + Sync {}

fn lazy<T: ?Sized + 'static>(name: &'static str, ty: &'static str) -> DependencyDescriptor {
    DependencyDescriptor {
        resolution: ResolutionMode::Lazy,
        ..one::<T>(name, ty)
    }
}

fn on_a() -> Vec<DependencyDescriptor> {
    vec![one::<PlannedA>("a", "PlannedA")]
}

fn on_a_renamed_twice() -> Vec<DependencyDescriptor> {
    vec![
        one::<PlannedA>("first", "PlannedA"),
        one::<PlannedA>("second", "PlannedA"),
    ]
}

fn on_a_lazily() -> Vec<DependencyDescriptor> {
    vec![lazy::<PlannedA>("a", "PlannedA")]
}

fn on_b() -> Vec<DependencyDescriptor> {
    vec![one::<PlannedB>("b", "PlannedB")]
}

fn on_a_and_b() -> Vec<DependencyDescriptor> {
    vec![
        one::<PlannedB>("b", "PlannedB"),
        one::<PlannedA>("a", "PlannedA"),
    ]
}

fn on_planned() -> Vec<DependencyDescriptor> {
    vec![one::<dyn Planned>("planned", "Planned")]
}

fn on_every_planned() -> Vec<DependencyDescriptor> {
    vec![DependencyDescriptor {
        cardinality: Cardinality::Collection,
        ..one::<dyn Planned>("planned", "Planned")
    }]
}

fn on_planned_lazily() -> Vec<DependencyDescriptor> {
    vec![lazy::<dyn Planned>("planned", "Planned")]
}

fn entry(
    key: &'static str,
    scope: ScopeKey,
    targets: fn() -> Vec<DependencyDescriptor>,
) -> EntryDescriptor {
    EntryDescriptor {
        key: EntryKey::new(key).expect("valid test entry key"),
        scope,
        targets,
        owner: None,
        source: None,
    }
}

/// Every child entry shape these tests plan, plus one sibling entry.
fn entries() -> Vec<EntryDescriptor> {
    vec![
        entry("test/a", Child::KEY, on_a),
        entry("test/a-renamed", Child::KEY, on_a_renamed_twice),
        entry("test/a-lazy", Child::KEY, on_a_lazily),
        entry("test/a-sibling", Sibling::KEY, on_a),
        entry("test/b", Child::KEY, on_b),
        entry("test/both", Child::KEY, on_a_and_b),
        entry("test/planned", Child::KEY, on_planned),
        entry("test/planned-all", Child::KEY, on_every_planned),
        entry("test/planned-lazy", Child::KEY, on_planned_lazily),
    ]
}

/// The child components and their [`Planned`] provider, with `extra` components and
/// providers after them.
fn generation(
    extra: &[ComponentDescriptor],
    extra_providers: Vec<ProviderDescriptor>,
) -> (Vec<ComponentDescriptor>, ProviderSelectionModel) {
    let mut descriptors = vec![
        descriptor::<PlannedA>("planned-a", "PlannedA", Child::KEY, empty_factory),
        descriptor::<PlannedB>("planned-b", "PlannedB", Child::KEY, empty_factory),
        descriptor::<ChildPlanned>("child-planned", "ChildPlanned", Child::KEY, empty_factory),
    ];
    let mut providers = vec![provider::<ChildPlanned, dyn Planned>("child", 0, &[])];

    descriptors.extend_from_slice(extra);
    providers.extend(extra_providers);

    let selection = selection_with(&descriptors, providers);

    (descriptors, selection)
}

fn plan(
    descriptors: &[ComponentDescriptor],
    selection: &ProviderSelectionModel,
    parallelism: Parallelism,
    base: Option<PlanBase<'_>>,
) -> ScopePlan {
    let topology = topology();
    let planning = EntryPlanning {
        selection,
        topology: &topology,
        unplanned: UnplannedConstruction::Deny,
        parallelism,
    };

    ScopePlan::partition_from(descriptors, &entries(), &planning, base)
        .expect("the test generation plans")
}

fn id(key: &'static str) -> EntryId {
    let table = EntryTable::new(&entries(), &topology()).expect("the test entries validate");

    table
        .id(&EntryKey::new(key).expect("valid test entry key"))
        .expect("the entry is registered")
}

fn shared<'a>(plan: &'a ScopePlan, key: &'static str) -> &'a Arc<EntryPlan> {
    plan.layouts
        .shared_entry(id(key))
        .expect("the entry has a plan")
}

#[test]
fn entries_with_one_signature_share_one_plan() {
    let (descriptors, selection) = generation(&[], Vec::new());

    let plan = plan(&descriptors, &selection, Parallelism::available(), None);

    assert!(
        Arc::ptr_eq(shared(&plan, "test/a"), shared(&plan, "test/a-renamed")),
        "target names and repeats do not split a signature"
    );
    assert!(
        !Arc::ptr_eq(shared(&plan, "test/a"), shared(&plan, "test/a-lazy")),
        "the resolution mode is part of the signature"
    );
    assert!(
        !Arc::ptr_eq(shared(&plan, "test/a"), shared(&plan, "test/a-sibling")),
        "the scope is part of the signature"
    );
    assert!(
        !Arc::ptr_eq(
            shared(&plan, "test/planned"),
            shared(&plan, "test/planned-all")
        ),
        "the cardinality is part of the signature"
    );
}

#[test]
fn parallel_planning_matches_sequential_planning() {
    let (descriptors, selection) = generation(&[], Vec::new());
    let keys = entries().into_iter().map(|entry| entry.key.as_str());

    let sequential = plan(
        &descriptors,
        &selection,
        Parallelism::new(usize::MAX, 1),
        None,
    );
    let parallel = plan(&descriptors, &selection, Parallelism::new(0, 4), None);

    for key in keys {
        let id = id(key);

        assert_eq!(
            sequential.layouts.entry(id),
            parallel.layouts.entry(id),
            "entry `{key}` plans alike on one thread and on four"
        );
    }

    for (scope, order) in &sequential.orders {
        assert_eq!(
            order
                .iter()
                .map(|component| component.id)
                .collect::<Vec<_>>(),
            parallel.orders[scope]
                .iter()
                .map(|component| component.id)
                .collect::<Vec<_>>(),
            "every scope sorts alike on one thread and on four"
        );
    }
}

#[test]
fn a_selection_change_replans_only_the_entries_reading_it() {
    let (descriptors, selection) = generation(&[], Vec::new());
    let root_planned =
        descriptor::<RootPlanned>("root-planned", "RootPlanned", Singleton::KEY, empty_factory);
    let (candidate_descriptors, candidate_selection) = generation(
        &[root_planned],
        vec![provider::<RootPlanned, dyn Planned>("root", 0, &[])],
    );
    let base = plan(&descriptors, &selection, Parallelism::available(), None);

    let candidate = plan(
        &candidate_descriptors,
        &candidate_selection,
        Parallelism::available(),
        Some(PlanBase {
            layouts: &base.layouts,
            selection: &selection,
        }),
    );

    for key in [
        "test/a",
        "test/a-lazy",
        "test/a-sibling",
        "test/b",
        "test/both",
    ] {
        assert!(
            Arc::ptr_eq(shared(&base, key), shared(&candidate, key)),
            "entry `{key}` reads nothing the root provider changes, so its plan carries over"
        );
    }

    for key in ["test/planned", "test/planned-all", "test/planned-lazy"] {
        assert!(
            !Arc::ptr_eq(shared(&base, key), shared(&candidate, key)),
            "entry `{key}` reads `Planned`, whose providers changed, so it is planned again"
        );
    }
}

#[test]
fn a_changed_layout_replans_every_entry_of_its_scope() {
    let (descriptors, selection) = generation(&[], Vec::new());
    let late = descriptor::<PlannedLate>("planned-late", "PlannedLate", Child::KEY, empty_factory);
    let (candidate_descriptors, candidate_selection) = generation(&[late], Vec::new());
    let base = plan(&descriptors, &selection, Parallelism::available(), None);

    let candidate = plan(
        &candidate_descriptors,
        &candidate_selection,
        Parallelism::available(),
        Some(PlanBase {
            layouts: &base.layouts,
            selection: &selection,
        }),
    );

    assert!(
        !Arc::ptr_eq(shared(&base, "test/a"), shared(&candidate, "test/a")),
        "a child entry is planned again against the new child layout"
    );
    assert!(
        Arc::ptr_eq(
            shared(&base, "test/a-sibling"),
            shared(&candidate, "test/a-sibling")
        ),
        "the sibling layout is unchanged, so its entry carries over"
    );
}
