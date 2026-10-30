//! Choosing between a factory's synchronous and async construction paths.

use super::*;
use crate::ReadyComponentFactory;
use crate::test_support::component;

/// A component that records which factory path built it.
struct Built(&'static str);

fn built(path: &'static str) -> BoxedComponent {
    boxed_component("Built", Arc::new(Built(path)))
}

fn construct(
    _: &mut ComponentConstructionContext,
) -> Pin<Box<dyn Future<Output = crate::Result<BoxedComponent>> + Send + '_>> {
    Box::pin(async { Ok(built("async")) })
}

const fn recipe(construct_ready: Option<ReadyComponentFactory>) -> ComponentFactoryDescriptor {
    ComponentFactoryDescriptor {
        id: "built",
        construct,
        construct_ready,
        dependencies: no_dependencies,
        role: ComponentFactoryRole::Explicit,
    }
}

static READY: [ComponentFactoryDescriptor; 1] = [recipe(Some(|_| Ok(Some(built("ready")))))];
static NOT_READY: [ComponentFactoryDescriptor; 1] = [recipe(Some(|_| Ok(None)))];
static FAILING: [ComponentFactoryDescriptor; 1] =
    [recipe(Some(|_| Err(Error::MissingComponent("Built"))))];
static ASYNC_ONLY: [ComponentFactoryDescriptor; 1] = [recipe(None)];

/// Opens a child scope holding one `Built` made by `factories`, and returns the path that
/// built it.
async fn build_path(
    factories: fn() -> &'static [ComponentFactoryDescriptor],
) -> crate::Result<&'static str> {
    let mut descriptor = component::<Built>("built", factories);

    descriptor.scope = TestScope::KEY;

    let child = ScopeContainer::open_child(
        TestScope::KEY,
        root().await,
        registry(),
        &[descriptor],
        Vec::new(),
    )
    .await?;
    let built = child
        .resolve::<Arc<Built>>()
        .await?
        .expect("the opened scope holds the component");

    Ok(built.0)
}

#[tokio::test]
async fn a_ready_factory_builds_without_its_async_path() {
    let path = build_path(|| &READY).await.expect("the scope opens");

    assert_eq!(path, "ready");
}

#[tokio::test]
async fn a_factory_that_is_not_ready_falls_back_to_its_async_path() {
    let not_ready = build_path(|| &NOT_READY).await.expect("the scope opens");
    let async_only = build_path(|| &ASYNC_ONLY).await.expect("the scope opens");

    assert_eq!(
        not_ready, "async",
        "a dependency that is not ready defers to construct"
    );
    assert_eq!(
        async_only, "async",
        "a factory without a ready path uses construct"
    );
}

#[tokio::test]
async fn a_ready_factory_error_fails_the_build() {
    let error = build_path(|| &FAILING)
        .await
        .expect_err("the ready path's error is the build's error");

    assert!(matches!(error, Error::MissingComponent("Built")));
}
