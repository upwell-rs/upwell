//! Reading the head under `deny_unplanned`: filling the seed is not a construction an entry must
//! declare, while building an undeclared component that reads it still fails.

use upwell::axum::axum::middleware::from_fn;

use super::*;

#[tokio::test]
async fn an_undeclared_seed_read_is_allowed_and_an_undeclared_build_is_not() {
    let meta = router(app().deny_unplanned().layer(from_fn(read_meta))).await;
    let built = router(app().deny_unplanned().layer(from_fn(read_reader))).await;

    let meta = get(&meta, "/tenant/undeclared").await;
    let built_undeclared = get(&built, "/tenant/undeclared").await;
    let built_declared = get(&built, "/tenant/declared").await;

    assert_eq!(meta.status(), StatusCode::OK);
    assert_eq!(seen(&meta), Some("original"));
    assert_eq!(built_undeclared.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(built_declared.status(), StatusCode::OK);
    assert_eq!(seen(&built_declared), Some("original"));
}
