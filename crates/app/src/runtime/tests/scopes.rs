//! Scope opening: declared scopes, typed parents, entries, seeds and generation pinning.

use upwell_core::{EntryKey, TypeDescriptor};
use upwell_di::BoxedComponent;

use super::*;
use crate::EntryDescriptor;
use crate::runtime::opener::validate_seeds;

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

fn visitor_destination() -> HashMap<TypeId, SeedDestination> {
    HashMap::from([(
        TypeId::of::<Visitor>(),
        SeedDestination {
            scope: SESSION_ID,
            type_name: std::any::type_name::<Visitor>(),
        },
    )])
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
async fn a_multi_parent_scope_opens_under_each_typed_parent() {
    let (runtime, _) = build_runtime(HashMap::new()).await;
    let shared = runtime.opener::<Shared>().expect("shared is declared");
    let session = open_session(&runtime).await;

    let under_root = shared
        .builder()
        .open_default()
        .await
        .expect("shared opens under the root");
    let under_session = shared
        .builder()
        .under(&session)
        .open_default()
        .await
        .expect("shared opens under a session");

    assert_eq!(under_root.container().scope(), Shared::KEY);
    assert_eq!(under_session.container().scope(), Shared::KEY);
}

#[tokio::test]
async fn a_parent_from_another_runtime_is_rejected() {
    let (runtime, _) = build_runtime(HashMap::new()).await;
    let (foreign, _) = build_runtime(HashMap::new()).await;
    let foreign_session = open_session(&foreign).await;

    let error = expect_app_error(
        runtime
            .opener::<Request>()
            .expect("request is declared")
            .builder()
            .under(&foreign_session)
            .open_default()
            .await,
        "a foreign runtime's session was accepted as a parent",
    );

    assert!(matches!(
        error,
        Error::ForeignScopeParent {
            child: REQUEST_ID,
            parent: SESSION_ID,
        }
    ));
}

#[tokio::test]
async fn an_opened_scope_takes_its_prepared_metadata() {
    let (runtime, _) = build_runtime(HashMap::new()).await;
    let session = open_session(&runtime).await;

    let request = runtime
        .opener::<RequestAlias>()
        .expect("the alias shares the request's identity")
        .builder()
        .under(&session)
        .open_default()
        .await
        .expect("request opens by stable identity");

    assert_eq!(request.container().scope().id(), REQUEST_ID);
    assert_eq!(request.container().scope().name(), Request::NAME);
    assert_eq!(request.container().scope().rank(), Request::RANK);
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
    let entries = [EntryDescriptor::new::<Session, ()>(key("test/login"))];
    let (runtime, _) = build_runtime_with_entries(visitor_destination(), &entries).await;
    let sessions = runtime.opener::<Session>().expect("session is declared");
    let login = sessions
        .entry(key("test/login"))
        .expect("the session entry resolves");

    let unseeded = sessions
        .builder()
        .open(&login)
        .await
        .expect("a seed slot no dependency needs may stay empty");
    let seeded = sessions
        .builder()
        .seed(Visitor(7))
        .open(&login)
        .await
        .expect("the entry opens its scope");
    let visitor = |session: &OpenScope<Session>| {
        session
            .container()
            .resolvers()
            .get_arc::<upwell_di::ComponentSource>()
            .expect("an open scope exposes its component source")
            .resolve::<Visitor>()
    };

    assert_eq!(visitor(&unseeded), None);
    assert_eq!(
        visitor(&seeded),
        Some(Visitor(7)),
        "the seed fills its slot"
    );
}

#[tokio::test]
async fn seeds_are_checked_against_the_declared_slots() {
    /// A seed absent from the application registry.
    struct Unknown;

    let seeds = |types: &[TypeDescriptor]| {
        types
            .iter()
            .map(|ty| BoxedComponent {
                ty: *ty,
                value: Box::new(()),
            })
            .collect::<Vec<_>>()
    };
    let visitor = TypeDescriptor::of::<Visitor>("Visitor");
    let (runtime, _) = build_runtime(visitor_destination()).await;
    let view = runtime.view();

    let declared = validate_seeds(&view, SESSION_ID, &seeds(&[visitor]));
    let elsewhere = validate_seeds(&view, REQUEST_ID, &seeds(&[visitor]));
    let unknown = validate_seeds(
        &view,
        SESSION_ID,
        &seeds(&[TypeDescriptor::of::<Unknown>("Unknown")]),
    );
    let duplicate = validate_seeds(&view, SESSION_ID, &seeds(&[visitor, visitor]));

    assert!(declared.is_ok());
    assert!(matches!(
        elsewhere,
        Err(Error::InvalidSeedDestination {
            expected: SESSION_ID,
            actual: REQUEST_ID,
            ..
        })
    ));
    assert!(matches!(
        unknown,
        Err(Error::UnregisteredSeed {
            scope: SESSION_ID,
            ..
        })
    ));
    assert!(matches!(
        duplicate,
        Err(Error::DuplicateSeedType {
            scope: SESSION_ID,
            ..
        })
    ));
}

#[tokio::test]
async fn a_child_runs_on_its_parent_generation_after_publication() {
    let (runtime, _) = build_runtime(HashMap::new()).await;
    let requests = runtime.opener::<Request>().expect("request is declared");
    let old_session = open_session(&runtime).await;

    let committed = publish_next(&runtime).await;
    let old_request = requests
        .builder()
        .under(&old_session)
        .open_default()
        .await
        .expect("request opens under the old session");
    let new_session = open_session(&runtime).await;

    assert_eq!(
        runtime.scope_generation(old_session.container()),
        Some(RuntimeGenerationId::INITIAL)
    );
    assert_eq!(
        runtime.scope_generation(old_request.container()),
        Some(RuntimeGenerationId::INITIAL),
        "a child runs on the generation its parent opened under"
    );
    assert_eq!(
        runtime.scope_generation(new_session.container()),
        Some(committed.id()),
        "a scope opened under the root runs on the current generation"
    );
    assert_eq!(committed.id(), RuntimeGenerationId::new(1));
}

#[tokio::test]
async fn a_durable_scope_reports_the_generations_it_fell_behind() {
    let (runtime, _) = build_runtime(HashMap::new()).await;
    let session = open_session(&runtime).await;

    assert_eq!(runtime.generations_behind(session.container()), Some(0));

    publish_next(&runtime).await;
    publish_next(&runtime).await;

    let request = runtime
        .opener::<Request>()
        .expect("request is declared")
        .builder()
        .under(&session)
        .open_default()
        .await
        .expect("request opens under the old session");
    let fresh = open_session(&runtime).await;

    assert_eq!(Session::KEY.generations(), ScopeGenerations::Durable);
    assert_eq!(Request::KEY.generations(), ScopeGenerations::Pinned);
    assert_eq!(runtime.generations_behind(session.container()), Some(2));
    assert_eq!(runtime.generations_behind(request.container()), Some(2));
    assert_eq!(runtime.generations_behind(fresh.container()), Some(0));
}
