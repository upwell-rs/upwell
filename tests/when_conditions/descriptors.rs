//! The static descriptors `#[when]` lowers to: one callback node per attribute with its
//! declared inputs and source metadata, combined under `All` in id order.

use upwell::{
    ComponentDescriptor, ConditionCallback, ConditionDescriptor, ConditionInputId,
    ConditionPredicate, ConfigBindingId, Descriptor, DescriptorSource,
};

use upwell::condition::ConditionPurity;

use crate::app::{
    AuditExporter, AuditTransport, AuthConfig, CustomAuthenticator, NamedCondition,
    ReorderedAuditExporter, TrustedCondition,
};

const FIXTURE: &str = include_str!("app.rs");

const FIXTURE_FILE: &str = "tests/when_conditions/app.rs";

fn condition<T: Descriptor<ComponentDescriptor>>() -> &'static ConditionDescriptor {
    T::DESCRIPTOR
        .condition
        .expect("the component carries a condition")
}

fn callback(node: &ConditionDescriptor) -> &'static ConditionCallback {
    match node.predicate {
        ConditionPredicate::Callback(callback) => callback,

        other => panic!("expected a callback node, got {other:?}"),
    }
}

fn nodes(root: &ConditionDescriptor) -> &'static [ConditionDescriptor] {
    match root.predicate {
        ConditionPredicate::All(nodes) => nodes,

        other => panic!("expected an `All` root, got {other:?}"),
    }
}

/// The 1-based line and column of the first occurrence of `needle` in the fixture.
fn location(needle: &str) -> (u32, u32) {
    let (index, line) = FIXTURE
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains(needle))
        .expect("the needle occurs in the fixture");
    let column = line.find(needle).expect("found above") + 1;

    (
        u32::try_from(index + 1).expect("line fits"),
        u32::try_from(column).expect("column fits"),
    )
}

fn assert_source(source: DescriptorSource, needle: &str) {
    let (line, column) = location(needle);

    assert!(
        source.file.replace('\\', "/").ends_with(FIXTURE_FILE),
        "the source names the fixture, got {}",
        source.file
    );
    assert_eq!((source.line, source.column), (line, column), "{needle}");
}

fn auth_binding() -> ConditionInputId {
    ConditionInputId::Binding(ConfigBindingId::of::<AuthConfig>("auth"))
}

#[test]
fn a_single_condition_is_one_callback_node_with_its_source() {
    let root = condition::<CustomAuthenticator>();
    let callback = callback(root);
    let expression = callback
        .expression
        .expect("the node carries its expression");

    assert!(root.id.starts_with("when-"), "content id, got {}", root.id);
    assert_source(root.source, "#[when(AuthConfig.mode == AuthMode::Custom)]");
    assert_eq!(callback.kind, "when");
    assert_eq!(callback.inputs, [auth_binding()]);
    assert_eq!(expression.text, "AuthConfig.mode == AuthMode::Custom");
    assert_eq!(expression.reads.len(), 1);
    assert_eq!(expression.reads[0].input, auth_binding());
    assert_eq!(expression.reads[0].field, "mode");
    assert_source(
        expression.reads[0].source,
        "AuthConfig.mode == AuthMode::Custom)]",
    );
}

#[test]
fn stacked_conditions_combine_under_all_in_id_order() {
    let root = condition::<AuditExporter>();
    let nodes = nodes(root);
    let ids = nodes.iter().map(|node| node.id).collect::<Vec<_>>();
    let mut sorted = ids.clone();

    sorted.sort_unstable();

    assert_eq!(root.id, "when");
    assert_eq!(ids, sorted, "nodes are ordered by id");

    let typed = nodes
        .iter()
        .map(callback)
        .find(|callback| callback.inputs.contains(&auth_binding()))
        .expect("the typed node");
    let raw = nodes
        .iter()
        .map(callback)
        .find(|callback| callback.inputs == [ConditionInputId::Raw("features.audit_export")])
        .expect("the raw node");
    let typed_reads = typed.expression.expect("typed expression").reads;

    assert_eq!(
        typed.inputs,
        [
            auth_binding(),
            ConditionInputId::Component("audittransport")
        ]
    );
    assert_eq!(
        typed_reads
            .iter()
            .map(|read| (read.input, read.field))
            .collect::<Vec<_>>(),
        [
            (auth_binding(), "audit"),
            (ConditionInputId::Component("audittransport"), ""),
        ]
    );
    assert_eq!(
        raw.expression.expect("raw expression").text,
        "resolve(\"features.audit_export\") == Some(true)"
    );
}

#[test]
fn declaration_order_does_not_change_the_descriptor() {
    let written = nodes(condition::<AuditExporter>());
    let reordered = nodes(condition::<ReorderedAuditExporter>());

    assert_eq!(
        written.iter().map(|node| node.id).collect::<Vec<_>>(),
        reordered.iter().map(|node| node.id).collect::<Vec<_>>(),
    );

    for (left, right) in written.iter().zip(reordered) {
        assert_eq!(callback(left).inputs, callback(right).inputs);
    }
}

#[test]
fn an_explicit_id_overrides_the_content_id() {
    let root = condition::<NamedCondition>();
    let expression = callback(root).expression.expect("expression");

    assert_eq!(root.id, "custom-mode");
    assert_eq!(expression.text, "AuthConfig.mode == AuthMode::Custom");
    assert_eq!(
        callback(condition::<CustomAuthenticator>()).inputs,
        callback(root).inputs
    );
}

#[test]
fn subset_expressions_are_pure_and_others_trusted() {
    let pure = [
        callback(condition::<CustomAuthenticator>()),
        callback(condition::<NamedCondition>()),
        callback(condition::<AuditTransport>()),
    ]
    .into_iter()
    .chain(nodes(condition::<AuditExporter>()).iter().map(callback));

    for callback in pure {
        assert_eq!(callback.purity, ConditionPurity::Pure);
    }

    assert_eq!(
        callback(condition::<TrustedCondition>()).purity,
        ConditionPurity::Trusted
    );
}
