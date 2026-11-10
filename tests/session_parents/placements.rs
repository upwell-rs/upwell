//! A component placed in a session and in another application scope: an optional edge from a
//! request reads the session's placement, whatever order the placements were registered in. A
//! component placed in two scopes that each enclose the request on some chain is rejected, since
//! which one it reads would depend on the chain.
#![allow(dead_code)]

use super::*;

/// A scope under the root that requests never open under, ranked below `HttpRequest`.
#[scope(parents = [Root], rank = 50)]
pub struct Batch;

/// A scope under the root that requests never open under, ranked above `HttpRequest`.
#[scope(parents = [Root], rank = 250)]
pub struct Archive;

/// A second scope requests may open under, beside [`Session`].
#[scope(parents = [Root], generations = durable, children = [HttpRequest])]
pub struct Workspace;

/// Declares a component labelled by the scope that built it, placed in `$first` by its
/// `#[component]` and in `$second` by an `#[init(scope = ..)]` factory, so its placements are
/// registered in that order.
macro_rules! placed_twice {
    ($name:ident, $first:ident, $second:ident) => {
        /// A component placed in two scopes, labelled by the scope that built it.
        #[component(scope = $first)]
        struct $name {
            #[default]
            label: &'static str,
        }

        #[methods]
        impl $name {
            #[init]
            fn in_first() -> Self {
                Self {
                    label: stringify!($first),
                }
            }
        }

        #[methods]
        impl $name {
            #[init(scope = $second)]
            fn in_second() -> Self {
                Self {
                    label: stringify!($second),
                }
            }
        }
    };
}

placed_twice!(BatchFirst, Batch, Session);
placed_twice!(SessionFirst, Session, Batch);
placed_twice!(ArchiveFirst, Archive, Session);
placed_twice!(Shared, Session, Workspace);

/// The label of an optional placed component, or `none`.
fn label_of<T>(value: Option<&Arc<T>>, label: impl Fn(&T) -> &'static str) -> &'static str {
    value.map_or("none", |value| label(value))
}

/// A request component reading every twice-placed cache optionally.
#[component(scope = HttpRequest)]
struct CacheView {
    batch_first: Option<Arc<BatchFirst>>,
    session_first: Option<Arc<SessionFirst>>,
    archive_first: Option<Arc<ArchiveFirst>>,
}

impl CacheView {
    fn describe(&self) -> String {
        [
            label_of(self.batch_first.as_ref(), |cache| cache.label),
            label_of(self.session_first.as_ref(), |cache| cache.label),
            label_of(self.archive_first.as_ref(), |cache| cache.label),
        ]
        .join(",")
    }
}

/// A request component reading the cache placed in both scopes requests open under.
#[component(scope = HttpRequest)]
struct SharedView {
    shared: Option<Arc<Shared>>,
}

/// A route reporting what [`CacheView`] resolved.
#[controller(path = "/caches")]
struct CacheController;

#[handlers]
impl CacheController {
    #[get("/view")]
    async fn view(&self, Inject(view): Inject<Arc<CacheView>>) -> String {
        view.describe()
    }

    #[get("/param")]
    async fn param(&self, Inject(cache): Inject<Option<Arc<BatchFirst>>>) -> String {
        label_of(cache.as_ref(), |cache| cache.label).to_owned()
    }
}

/// [`with_sessions`] with every scope and twice-placed component of this module.
fn with_placements(dir: &Path) -> AppBuilder<Axum> {
    with_sessions(dir)
        .scope::<Batch>()
        .scope::<Archive>()
        .scope::<Workspace>()
        .controller::<CacheController>()
        .component::<BatchFirst>()
        .component::<SessionFirst>()
        .component::<ArchiveFirst>()
        .component::<CacheView>()
}

#[tokio::test]
async fn a_request_reads_the_session_placement_in_either_registration_order() {
    let dir = config_dir(1);
    let (router, _runtime) = build(with_placements(dir.path())).await;

    assert_eq!(
        get(&router, "/caches/view", Some("ned")).await,
        "Session,Session,Session",
        "a request under a session reads the session's placement"
    );
    assert_eq!(
        get(&router, "/caches/view", None).await,
        "none,none,none",
        "a request under the root reads none"
    );
    assert_eq!(get(&router, "/caches/param", Some("ned")).await, "Session");
    assert_eq!(get(&router, "/caches/param", None).await, "none");
}

#[tokio::test]
async fn a_component_placed_in_both_parents_is_rejected() {
    let dir = config_dir(1);
    let error = match with_placements(dir.path())
        .component::<Shared>()
        .component::<SharedView>()
        .build()
        .await
    {
        Ok(_) => panic!("the app is rejected"),
        Err(error) => format!("{error:?}"),
    };

    assert!(error.contains("AmbiguousEnclosingPlacement"), "{error}");
    assert!(
        error.contains("Session") && error.contains("Workspace"),
        "{error}"
    );
}
