//! A deferred cycle left out of a scope's opening, built on demand as one group.

use super::*;
use crate::Deferred;

/// Half of a cycle, reaching [`Back`] once its scope hydrates it.
struct Forth {
    back: Deferred<Back>,
}

/// The other half of the cycle, depending on [`Forth`] directly.
struct Back {
    forth: Arc<Forth>,
}

test_component!(Forth, "forth");
test_component!(Back, "back");

async fn forth(back: Deferred<Back>) -> Forth {
    Forth { back }
}

async fn back(forth: Arc<Forth>) -> Back {
    Back { forth }
}

scoped!(forth_descriptor, Forth, "forth", forth);
scoped!(back_descriptor, Back, "back", back);

fn cycle_layout() -> Arc<crate::SlotLayout> {
    Arc::new(plan(&[forth_descriptor(), back_descriptor()]))
}

/// Checks that `forth` and `back` are the two halves of one cycle built in `scope`.
fn assert_one_cycle(scope: &Arc<ScopeContainer>, forth: &Arc<Forth>) {
    let back = scope
        .get::<Back>()
        .expect("the cycle's other half is built");

    assert!(Arc::ptr_eq(&back.forth, forth));
    assert!(Arc::ptr_eq(&forth.back.get(), &back));
}

#[tokio::test]
async fn an_entry_reaching_one_half_of_a_deferred_cycle_builds_both() {
    let layout = cycle_layout();
    let plan = entry_plan(&layout, &[<Arc<Forth> as FromContainer>::dependency()]);

    let scope = open(&layout, Some(&plan)).await;
    let forth = scope.get::<Forth>().expect("the targeted half is built");

    assert_eq!(plan.len(), 2);
    assert_one_cycle(&scope, &forth);
}

#[tokio::test]
async fn a_deferred_cycle_left_out_of_the_opening_is_built_on_demand_as_a_unit() {
    let layout = cycle_layout();
    let scope = open(&layout, Some(&entry_plan(&layout, &[]))).await;

    let forth = scope
        .resolve::<Arc<Forth>>()
        .await
        .expect("the cycle builds on demand")
        .expect("the targeted half is built");

    assert_one_cycle(&scope, &forth);
}
