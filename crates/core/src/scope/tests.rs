use super::{InvalidScopeIdReason, Scope, ScopeId, ScopeKey, Singleton, Transient};
use crate::NamespacedIdType;

const FIRST_ID: ScopeId = crate::namespaced_id!(ScopeId, "test/first");
const SECOND_ID: ScopeId = crate::namespaced_id!(ScopeId, "test/second");

crate::namespaced_id_type!(
    /// A second ID category used to prove runtime cross-type namespace checks.
    struct OtherId,
    "test other"
);

/// A test scope sharing its display label and rank with [`SecondScope`].
struct FirstScope;

impl Scope for FirstScope {
    const ID: ScopeId = FIRST_ID;
    const RANK: u8 = 100;
    const NAME: &'static str = "Shared";
}

/// A test scope sharing its display label and rank with [`FirstScope`].
struct SecondScope;

impl Scope for SecondScope {
    const ID: ScopeId = SECOND_ID;
    const RANK: u8 = 100;
    const NAME: &'static str = "Shared";
}

#[test]
fn scope_ids_validate_namespaced_ascii_paths() {
    let valid = ScopeId::new("acme/rpc.request-v2").expect("valid scope ID");
    let missing = ScopeId::new("request").expect_err("namespace is required");
    let uppercase = ScopeId::new("Acme/request").expect_err("uppercase is rejected");
    let empty = ScopeId::new("acme//request").expect_err("empty segment is rejected");
    let unicode = ScopeId::new("acme/routér").expect_err("unicode is rejected");

    assert_eq!(valid.as_str(), "acme/rpc.request-v2");
    assert_eq!(valid.as_ref(), "acme/rpc.request-v2");
    assert_eq!(valid.namespace(), "acme");
    assert_eq!(valid.name(), "rpc.request-v2");
    assert_eq!(valid.local_path(), "rpc.request-v2");
    assert_eq!(valid.to_string(), "acme/rpc.request-v2");
    assert_eq!(missing.value(), "request");
    assert_eq!(missing.reason(), InvalidScopeIdReason::MissingNamespace);
    assert_eq!(
        uppercase.reason(),
        InvalidScopeIdReason::InvalidSegmentStart
    );
    assert_eq!(empty.reason(), InvalidScopeIdReason::MissingNamespace);
    assert_eq!(unicode.reason(), InvalidScopeIdReason::InvalidCharacter);
    assert_eq!(
        unicode.to_string(),
        "invalid scope id 'acme/routér': segments may contain only lowercase ASCII letters, digits, '.', '_', and '-'"
    );
}

#[test]
fn shared_id_trait_inspects_and_compares_categories_without_erasing_identity() {
    let first = ScopeId::new("acme/platform/request").expect("valid scope ID");
    let second = OtherId::new("acme/platform/connection").expect("valid other ID");
    let other = OtherId::new("other/platform/request").expect("valid other ID");

    assert_eq!(second.as_str(), "acme/platform/connection");
    assert_eq!(second.name(), "platform/connection");
    assert_eq!(second.local_path(), "platform/connection");
    assert!(first.is_in_namespace("acme"));
    assert!(first.shares_namespace_with(&second));
    assert!(!first.shares_namespace_with(&other));
}

#[test]
fn stable_identity_is_distinct_from_display_label_and_rank() {
    let first = FirstScope::KEY;
    let second = SecondScope::KEY;

    assert_eq!(first.name(), second.name());
    assert_eq!(first.rank(), second.rank());
    assert_ne!(first.id(), second.id());
    assert_eq!(first.id(), FIRST_ID);
    assert_eq!(second.id(), SECOND_ID);
}

#[test]
fn framework_scopes_have_reserved_stable_ids() {
    let singleton = Singleton::KEY;
    let transient = Transient::KEY;

    assert_eq!(singleton.id().as_str(), "upwell/singleton");
    assert_eq!(transient.id().as_str(), "upwell/transient");
    assert!(transient.is_transient());
}

#[test]
fn scope_keys_compare_by_identity_and_capture_type_metadata() {
    let first = ScopeKey::of::<FirstScope>();

    assert_eq!(first, FirstScope::KEY);
    assert_ne!(first, SecondScope::KEY);
    assert_eq!(first.name(), "Shared");
    assert_eq!(first.rank(), 100);
    assert!(!first.is_transient());
    assert_eq!(Singleton::KEY.rank(), u8::MAX);
}
