//! Target validation against the entry's scope chain and the config bindings.

use super::*;

#[test]
fn targets_resolve_from_the_entry_scope_and_every_ancestor() {
    let registry = registry(vec![
        EntryDescriptor::new::<Request, (Arc<Shared>, Arc<SessionStore>, Arc<RequestData>)>(key(
            "test/request",
        )),
        EntryDescriptor::new::<Singleton, (Arc<Shared>, Option<Arc<Unregistered>>)>(key(
            "test/root",
        )),
    ]);

    validate(&registry).expect("every target is reachable");
}

#[test]
fn targets_below_the_entry_scope_are_unreachable() {
    let registry = registry(vec![EntryDescriptor::new::<Session, (Arc<RequestData>,)>(
        key("test/session"),
    )]);

    let error = entry_error(validate(&registry));

    assert!(matches!(
        &error,
        EntryError::UnreachableDependency { scope: SESSION_ID, provider_scopes, .. }
            if **provider_scopes == [REQUEST_ID]
    ));
    assert_eq!(
        error.to_string(),
        format!(
            "entry 'test/session' injects '{}', but its providers live in 'test/entry-request', \
             which scope 'test/entry-session' cannot reach",
            std::any::type_name::<RequestData>()
        )
    );
}

#[test]
fn unprovided_and_tied_targets_are_rejected() {
    let missing = registry(vec![EntryDescriptor::new::<Session, (Arc<Unregistered>,)>(
        key("test/missing"),
    )]);
    let mut tied = registry(vec![EntryDescriptor::new::<Session, (Arc<dyn Greeter>,)>(
        key("test/tied"),
    )]);

    tied.components.push(manual::<English>(Singleton::KEY));
    tied.components.push(manual::<Swedish>(Singleton::KEY));
    tied.providers = vec![greeter::<English>(), greeter::<Swedish>()];

    let missing = entry_error(validate(&missing));
    let tied = entry_error(validate(&tied));

    assert!(matches!(missing, EntryError::MissingDependency { .. }));
    assert!(matches!(
        &tied,
        EntryError::AmbiguousDependency { candidates, .. }
            if **candidates == [String::from("english"), String::from("swedish")]
    ));
}

#[test]
fn config_targets_need_exactly_one_binding() {
    let entries = vec![EntryDescriptor::new::<Session, (Cfg<Limits>,)>(key(
        "test/config",
    ))];
    let unbound = registry(entries.clone());
    let mut bound = registry(entries.clone());
    let mut ambiguous = registry(entries);

    bound.config_bindings = vec![ConfigBinding::of::<Limits>("limits")];
    ambiguous.config_bindings = vec![
        ConfigBinding::of::<Limits>("limits"),
        ConfigBinding::of::<Limits>("other"),
    ];

    let unbound = entry_error(validate(&unbound));
    let ambiguous = entry_error(validate(&ambiguous));

    validate(&bound).expect("the sole binding satisfies the target");
    assert!(matches!(
        unbound,
        EntryError::MissingConfig { path: None, .. }
    ));
    assert!(matches!(
        &ambiguous,
        EntryError::AmbiguousConfig { paths, .. } if paths.len() == 2
    ));
}

#[test]
fn fresh_targets_need_a_reachable_scope_and_a_factory() {
    let unreachable = registry(vec![EntryDescriptor::new::<
        Session,
        (upwell_di::Fresh<Arc<RequestData>>,),
    >(key("test/fresh-unreachable"))]);
    let factoryless = registry(vec![EntryDescriptor::new::<
        Session,
        (upwell_di::Fresh<Arc<Shared>>,),
    >(key("test/fresh-factoryless"))]);

    let unreachable = entry_error(validate(&unreachable));
    let factoryless = entry_error(validate(&factoryless));

    assert!(matches!(
        unreachable,
        EntryError::FreshUnreachable {
            scope: REQUEST_ID,
            ..
        }
    ));
    assert!(matches!(
        &factoryless,
        EntryError::FreshWithoutFactory { component, .. } if component == "shared"
    ));
}
