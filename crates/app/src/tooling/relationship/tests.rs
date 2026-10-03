use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

use upwell_config::{ConfigManager, Toml};
use upwell_core::{Cardinality, DependencyDescriptor, ResolutionMode, TypeDescriptor};
use upwell_di::{
    BoxedComponent, ComponentConstructionContext, ComponentDescriptor, ComponentFactoryDescriptor,
    ProviderDescriptor, Singleton,
};
use upwell_tooling_schema::{DocumentIdentity, Relationship, RelationshipKind, ToolingDocument};

use super::merge_relationship_labels;
use crate::App;

struct AggregateConsumer;
struct AggregateProvider;

trait AggregateTrait: Send + Sync {}

impl AggregateTrait for AggregateProvider {}

fn fake_factory<'a>(
    _: &'a mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = upwell_di::Result<BoxedComponent>> + Send + 'a>> {
    Box::pin(async { unreachable!("projection tests do not construct components") })
}

fn no_dependencies() -> Vec<DependencyDescriptor> {
    Vec::new()
}

fn aggregate_dependencies() -> Vec<DependencyDescriptor> {
    vec![
        DependencyDescriptor {
            name: "QualifiedAggregateTrait",
            ty: TypeDescriptor::of::<dyn AggregateTrait>("AggregateTrait"),
            cardinality: Cardinality::One,
            optional: false,
            dynamic: false,
            qualifier: Some("shared"),
            config: false,
            resolution: ResolutionMode::Eager,
            observation: upwell_core::DependencyObservation::Snapshot,
        },
        DependencyDescriptor {
            name: "FreshAggregateTraits",
            ty: TypeDescriptor::of::<dyn AggregateTrait>("AggregateTrait"),
            cardinality: Cardinality::Collection,
            optional: false,
            dynamic: false,
            qualifier: None,
            config: false,
            resolution: ResolutionMode::Fresh,
            observation: upwell_core::DependencyObservation::Snapshot,
        },
    ]
}

static PROVIDER_FACTORIES: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: fake_factory,
    dependencies: no_dependencies,
    default: false,
}];

static CONSUMER_FACTORIES: [ComponentFactoryDescriptor; 1] = [ComponentFactoryDescriptor {
    id: "static",
    construct: fake_factory,
    dependencies: aggregate_dependencies,
    default: false,
}];

fn provider_factories() -> &'static [ComponentFactoryDescriptor] {
    &PROVIDER_FACTORIES
}

fn consumer_factories() -> &'static [ComponentFactoryDescriptor] {
    &CONSUMER_FACTORIES
}

fn erase_unreachable(_: &BoxedComponent) -> BoxedComponent {
    unreachable!("projection tests do not erase providers")
}

static PROVIDER_COMPONENT: ComponentDescriptor = ComponentDescriptor {
    id: "aggregate-provider",
    name: "AggregateProvider",
    ty: TypeDescriptor::of::<AggregateProvider>("AggregateProvider"),
    scope: &Singleton,
    condition: None,
    factories: provider_factories,
    hooks: upwell_hooks::no_hooks,
    generation_snapshot: None,
};

static CONSUMER_COMPONENT: ComponentDescriptor = ComponentDescriptor {
    id: "aggregate-consumer",
    name: "AggregateConsumer",
    ty: TypeDescriptor::of::<AggregateConsumer>("AggregateConsumer"),
    scope: &Singleton,
    condition: None,
    factories: consumer_factories,
    hooks: upwell_hooks::no_hooks,
    generation_snapshot: None,
};

static PROVIDER: ProviderDescriptor = ProviderDescriptor {
    trait_ty: TypeDescriptor::of::<dyn AggregateTrait>("AggregateTrait"),
    concrete_ty: TypeDescriptor::of::<AggregateProvider>("AggregateProvider"),
    qualifier: "shared",
    primary: true,
    priority: 0,
    ordering: &[],
    erase: erase_unreachable,
};

#[test]
fn exact_duplicates_are_idempotent_and_shared_labels_remain_scalar() {
    let mut labels = BTreeMap::from([
        (String::from("role"), String::from("resolved-provider")),
        (String::from("cardinality"), String::from("one")),
    ]);
    let duplicate = labels.clone();

    merge_relationship_labels(&RelationshipKind::DependsOn, &mut labels, duplicate);

    assert_eq!(labels.len(), 2);
    assert_eq!(labels["role"], "resolved-provider");
    assert_eq!(labels["cardinality"], "one");
}

#[test]
fn conflicting_dependency_facts_are_sorted_and_schema_unique() {
    let mut labels = BTreeMap::from([
        (String::from("role"), String::from("resolved-provider")),
        (String::from("cardinality"), String::from("one")),
        (String::from("resolution"), String::from("eager")),
        (String::from("qualifier"), String::from("primary")),
    ]);

    merge_relationship_labels(
        &RelationshipKind::DependsOn,
        &mut labels,
        BTreeMap::from([
            (String::from("role"), String::from("resolved-provider")),
            (String::from("cardinality"), String::from("collection")),
            (String::from("resolution"), String::from("fresh")),
            (String::from("qualifier"), String::from("secondary")),
        ]),
    );
    merge_relationship_labels(
        &RelationshipKind::DependsOn,
        &mut labels,
        BTreeMap::from([
            (String::from("role"), String::from("resolved-provider")),
            (String::from("cardinality"), String::from("keyed")),
            (String::from("resolution"), String::from("deferred")),
        ]),
    );

    assert_eq!(labels["role"], "resolved-provider");
    let decisions: Vec<BTreeMap<String, String>> =
        serde_json::from_str(&labels["dependency-decisions"])
            .expect("dependency decisions deserialize");

    assert_eq!(
        decisions,
        [
            BTreeMap::from([
                (String::from("cardinality"), String::from("collection")),
                (String::from("qualifier"), String::from("secondary")),
                (String::from("resolution"), String::from("fresh")),
                (String::from("role"), String::from("resolved-provider")),
            ]),
            BTreeMap::from([
                (String::from("cardinality"), String::from("keyed")),
                (String::from("resolution"), String::from("deferred")),
                (String::from("role"), String::from("resolved-provider")),
            ]),
            BTreeMap::from([
                (String::from("cardinality"), String::from("one")),
                (String::from("qualifier"), String::from("primary")),
                (String::from("resolution"), String::from("eager")),
                (String::from("role"), String::from("resolved-provider")),
            ]),
        ]
    );

    let mut document = ToolingDocument::new(
        "0.20.0",
        DocumentIdentity {
            application: String::from("aggregation"),
            ..DocumentIdentity::default()
        },
        "test/protocol",
    );
    document.resources.extend([
        upwell_tooling_schema::Resource {
            id: String::from("component:consumer"),
            kind: upwell_tooling_schema::ResourceKind::Component,
            name: String::from("Consumer"),
            ..Default::default()
        },
        upwell_tooling_schema::Resource {
            id: String::from("provider:test"),
            kind: upwell_tooling_schema::ResourceKind::Provider,
            name: String::from("Provider"),
            ..Default::default()
        },
    ]);
    document.relationships.push(Relationship {
        kind: RelationshipKind::DependsOn,
        from: String::from("component:consumer"),
        to: String::from("provider:test"),
        labels,
    });

    assert!(document.validate().is_ok());
}

#[test]
fn projection_aggregates_dependencies_to_the_same_type_and_provider() {
    let mut builder = App::<()>::builder("relationship-aggregation")
        .config_source(ConfigManager::<Toml>::empty())
        .component_descriptor(&PROVIDER_COMPONENT)
        .component_descriptor(&CONSUMER_COMPONENT);

    builder.registry_mut().providers.push(PROVIDER);

    let document = builder
        .prepare()
        .expect("aggregation fixture prepares")
        .tooling_document()
        .expect("aggregation fixture projects");
    let provider = document
        .resources
        .iter()
        .find(|resource| {
            resource.kind == upwell_tooling_schema::ResourceKind::Provider
                && resource.labels.get("qualifier") == Some(&String::from("shared"))
        })
        .expect("provider resource exists");
    let provider_edge = document
        .relationships
        .iter()
        .find(|relationship| {
            relationship.kind == RelationshipKind::DependsOn
                && relationship.from == "component:aggregate-consumer"
                && relationship.to == provider.id
        })
        .expect("aggregated provider edge exists");
    let type_edge = document
        .relationships
        .iter()
        .find(|relationship| {
            relationship.kind == RelationshipKind::DependsOn
                && relationship.from == "component:aggregate-consumer"
                && relationship.to.starts_with("type:")
        })
        .expect("aggregated type edge exists");

    assert_eq!(provider_edge.labels["role"], "resolved-provider");

    for (edge, selected) in [(provider_edge, true), (type_edge, false)] {
        let decisions: Vec<BTreeMap<String, String>> =
            serde_json::from_str(&edge.labels["dependency-decisions"])
                .expect("dependency decisions deserialize");
        let tuples = decisions
            .iter()
            .map(|decision| {
                (
                    decision["cardinality"].as_str(),
                    decision["resolution"].as_str(),
                    decision.get("qualifier").map(String::as_str),
                    decision.get("role").map(String::as_str),
                    decision.get("selection-reason").map(String::as_str),
                    decision.get("requested-type").map(String::as_str),
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            tuples,
            if selected {
                vec![
                    (
                        "collection",
                        "fresh",
                        None,
                        Some("resolved-provider"),
                        Some("collection"),
                        Some(std::any::type_name::<dyn AggregateTrait>()),
                    ),
                    (
                        "one",
                        "eager",
                        Some("shared"),
                        Some("resolved-provider"),
                        Some("qualified"),
                        Some(std::any::type_name::<dyn AggregateTrait>()),
                    ),
                ]
            } else {
                vec![
                    ("collection", "fresh", None, None, None, None),
                    ("one", "eager", Some("shared"), None, None, None),
                ]
            }
        );
    }

    assert_eq!(
        document
            .relationships
            .iter()
            .filter(|relationship| {
                relationship.kind == RelationshipKind::DependsOn
                    && relationship.from == "component:aggregate-consumer"
                    && relationship.to == provider.id
            })
            .count(),
        1
    );
}
