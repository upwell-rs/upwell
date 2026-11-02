//! Generation pinning: an instance opened on a runtime generation reads that generation's
//! singletons for its whole life, even after a later transition republishes a root cell a
//! retained consumer holds.

use super::*;
use crate::layout::{EntryPlan, SlotLayout};
use crate::test_support::{component, test_component};
use crate::{Dep, Lazy, Live, LiveCell, dispatch_factory, factory_dependencies};

/// A singleton whose instance a transition replaces, identified by its generation.
struct Setting(u8);

/// A scoped component built on demand from the singleton.
struct Reader {
    setting: Arc<Setting>,
}

test_component!(Setting, "setting");
test_component!(Reader, "reader");

async fn reader(setting: Arc<Setting>) -> Reader {
    Reader { setting }
}

fn reader_factories() -> &'static [ComponentFactoryDescriptor] {
    const {
        &[ComponentFactoryDescriptor {
            id: "reader",
            construct: |cx| dispatch_factory(reader, cx),
            construct_ready: None,
            dependencies: || factory_dependencies(reader),
            role: ComponentFactoryRole::Explicit,
        }]
    }
}

fn setting_factories() -> &'static [ComponentFactoryDescriptor] {
    &[]
}

/// A scope type that spans commits, as a connection does.
struct Connection;

impl Scope for Connection {
    const ID: ScopeId = upwell_core::namespaced_id!(ScopeId, "test/connection");
    const RANK: u8 = 2;
    const NAME: &'static str = "Connection";
    const GENERATIONS: upwell_core::ScopeGenerations = upwell_core::ScopeGenerations::Durable;
}

/// The singleton, a reader in scope `S` reading it through a planned root edge, and the
/// layout of `S`.
fn reader_layout<S: Scope>() -> Arc<SlotLayout> {
    let setting = component::<Setting>("setting", setting_factories);
    let mut scoped = component::<Reader>("reader", reader_factories);

    scoped.scope = S::KEY;

    let selection = crate::registry::ProviderSelectionModel::new(
        &[setting, scoped],
        Vec::new(),
        HashMap::new(),
    )
    .expect("the components select no providers");

    Arc::new(
        SlotLayout::plan(
            S::KEY,
            &[],
            &[scoped],
            &HashMap::new(),
            &selection,
            |_, _| true,
        )
        .expect("the scope plans"),
    )
}

/// A root holding `Setting(1)` in a swappable cell, and the cell a retained consumer holds.
async fn generation_root() -> (Arc<ScopeContainer>, LiveCell) {
    let root = ScopeContainer::build_root(
        &[component::<Setting>("setting", setting_factories)],
        vec![boxed_component("Setting", Arc::new(Setting(1)))],
        ResolverSet::new(),
        registry(),
    )
    .await
    .expect("root builds");
    let held = LiveCell::of(
        root.store()
            .component(TypeId::of::<Setting>())
            .and_then(|boxed| boxed.value.downcast_ref::<Live<Setting>>())
            .expect("the root stores the singleton in a live cell"),
    );

    (root, held)
}

/// Republishes `held` with the next generation's `Setting(2)`, as a committed transition
/// rebinds a retained consumer's cell.
fn commit_next_generation(held: &LiveCell) {
    let next = LiveCell::of(&Live::new(Arc::new(Setting(2))));

    held.rebind_to(&next)
        .expect("both cells hold a setting")
        .commit();
}

/// Opens `layout` under `parent`, building only its eager set.
async fn open_on_demand(
    parent: &Arc<ScopeContainer>,
    layout: &Arc<SlotLayout>,
) -> Arc<ScopeContainer> {
    ScopeContainer::open_planned(
        Arc::clone(parent),
        parent.registry(),
        layout,
        Some(&EntryPlan::on_demand(layout)),
        Vec::new(),
        Arc::new(()),
    )
    .await
    .expect("the scope opens")
}

#[tokio::test]
async fn a_pinned_scope_reads_its_generation_after_a_commit_rebinds_the_root() {
    let (root, held) = generation_root().await;
    let layout = reader_layout::<TestScope>();
    let scope = open_on_demand(&root, &layout).await;
    let escaped = scope
        .extract::<Lazy<Arc<Setting>>>()
        .await
        .expect("a lazy handle escapes the scope");

    commit_next_generation(&held);

    let built = scope
        .resolve::<Arc<Reader>>()
        .await
        .expect("the reader builds on demand")
        .expect("the scope holds the reader");
    let read = scope
        .resolve::<Arc<Setting>>()
        .await
        .expect("the setting resolves")
        .expect("the root holds the setting");
    let lazy = escaped
        .get_or_create()
        .await
        .expect("the escaped handle resolves");
    let live = scope
        .resolve::<Dep<Setting>>()
        .await
        .expect("the setting resolves")
        .expect("the root holds the setting");

    assert_eq!(
        built.setting.0, 1,
        "a lazily built component reads its generation"
    );
    assert_eq!(read.0, 1, "a dynamic read sees its generation");
    assert_eq!(
        lazy.0, 1,
        "an escaped lazy handle resolves on its generation"
    );
    assert_eq!(
        live.snapshot().0,
        2,
        "a Dep follows the root's live binding, as the consumer holding it does"
    );
}

#[tokio::test]
async fn a_message_under_an_old_connection_reads_the_connection_generation() {
    let (root, held) = generation_root().await;
    let connection = open_on_demand(&root, &reader_layout::<Connection>()).await;

    commit_next_generation(&held);

    let message = open_on_demand(&connection, &reader_layout::<TestScope>()).await;
    let in_message = message
        .resolve::<Arc<Reader>>()
        .await
        .expect("the message's reader builds on demand")
        .expect("the message holds its reader");
    let in_connection = connection
        .resolve::<Arc<Reader>>()
        .await
        .expect("the connection's reader builds on demand")
        .expect("the connection holds its reader");

    assert_eq!(in_message.setting.0, 1);
    assert_eq!(in_connection.setting.0, 1);
}
