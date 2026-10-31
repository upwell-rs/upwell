//! The prepared scope opener: declared scopes, typed parents, entries and seeds.

use upwell_core::{EntryKey, Singleton};

use super::*;
use crate::{EntryDescriptor, OpensUnder};

impl OpensUnder<Singleton> for Session {}
impl OpensUnder<Session> for Request {}
impl OpensUnder<Singleton> for Shared {}
impl OpensUnder<Session> for Shared {}

/// A by-value seed declared for the session boundary.
#[derive(Clone, Debug, PartialEq)]
struct Visitor(u8);

impl upwell_di::Injectable for Visitor {
    type Target = Self;
    type Stored = Self;

    fn into_stored(self) -> Self {
        self
    }

    fn from_stored(stored: &Self) -> Self {
        stored.clone()
    }
}

fn key(value: &'static str) -> EntryKey {
    EntryKey::new(value).expect("valid test entry key")
}

#[tokio::test]
async fn an_opener_requires_a_declared_scope() {
    let (runtime, _) = build_runtime(HashMap::new()).await;

    let undeclared = expect_app_error(
        runtime.opener::<Other>(),
        "an opener was prepared for an undeclared scope",
    );

    assert!(matches!(
        undeclared,
        Error::UndeclaredScopeOpen { scope: OTHER_ID }
    ));
}

#[tokio::test]
async fn a_child_opens_under_its_typed_parent_on_the_parent_generation() {
    let (runtime, _) = build_runtime(HashMap::new()).await;
    let sessions = runtime.opener::<Session>().expect("session is declared");
    let requests = runtime.opener::<Request>().expect("request is declared");
    let session = sessions
        .builder()
        .open_default()
        .await
        .expect("session opens under the root");

    let current = publish_next(&runtime).await;
    let request = requests
        .builder()
        .under(&session)
        .open_default()
        .await
        .expect("request opens under its session");
    let shared = runtime
        .opener::<Shared>()
        .expect("shared is declared")
        .builder()
        .open_default()
        .await
        .expect("shared opens under the root");

    assert_eq!(request.container().scope(), Request::KEY);
    assert_eq!(
        runtime.scope_generation(request.container()),
        Some(RuntimeGenerationId::INITIAL),
        "a child runs on the generation its parent opened under"
    );
    assert_eq!(
        runtime.scope_generation(shared.container()),
        Some(current.id()),
        "a scope opened under the root runs on the current generation"
    );
}

#[tokio::test]
async fn an_opener_resolves_only_entries_of_its_scope() {
    let entries = [
        EntryDescriptor::new::<Request, ()>(key("test/get-user")),
        EntryDescriptor::new::<Session, ()>(key("test/login")),
    ];
    let (runtime, _) = build_runtime_with_entries(HashMap::new(), &entries).await;
    let requests = runtime.opener::<Request>().expect("request is declared");

    let get_user = requests
        .entry(key("test/get-user"))
        .expect("a request entry resolves");
    let login = expect_app_error(
        requests.entry(key("test/login")),
        "a session entry resolved for the request opener",
    );
    let unknown = expect_app_error(
        requests.entry(key("test/missing")),
        "an unregistered entry resolved",
    );

    assert_eq!(get_user.id().index(), 0);
    assert!(matches!(
        login,
        Error::EntryScopeMismatch {
            expected: REQUEST_ID,
            actual: SESSION_ID,
            ..
        }
    ));
    assert!(matches!(unknown, Error::UnknownEntry { .. }));
}

#[tokio::test]
async fn an_entry_opens_its_scope_with_the_supplied_seeds() {
    let destinations = HashMap::from([(
        TypeId::of::<Visitor>(),
        SeedDestination {
            scope: SESSION_ID,
            type_name: std::any::type_name::<Visitor>(),
        },
    )]);
    let entries = [EntryDescriptor::new::<Session, ()>(key("test/login"))];
    let (runtime, _) = build_runtime_with_entries(destinations, &entries).await;
    let sessions = runtime.opener::<Session>().expect("session is declared");
    let login = sessions
        .entry(key("test/login"))
        .expect("the session entry resolves");

    let session = sessions
        .builder()
        .seed(Visitor(7))
        .open(&login)
        .await
        .expect("the entry opens its scope");
    let visitor = session
        .container()
        .resolvers()
        .get_arc::<upwell_di::ComponentSource>()
        .expect("an open scope exposes its component source")
        .resolve::<Visitor>();

    assert_eq!(visitor, Some(Visitor(7)), "the seed fills its slot");
}
