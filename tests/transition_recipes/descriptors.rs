//! The descriptors the recipe attributes generate match the `factories!` builder form.

use upwell::{
    ComponentDescriptor, ComponentFactoryDescriptor, ComponentFactoryRole, Descriptor, factories,
};

use crate::app::{Banner, Greeter, Ledger};

/// The single `role` recipe the `#[methods]` block of `T` registered.
fn generated<T: Descriptor<ComponentDescriptor>>(
    role: ComponentFactoryRole,
) -> &'static ComponentFactoryDescriptor {
    let mut recipes = (T::DESCRIPTOR.factories)()
        .iter()
        .filter(|factory| factory.role == role);
    let recipe = recipes.next().expect("the attribute registered its recipe");

    assert!(recipes.next().is_none(), "one recipe per role");

    recipe
}

fn assert_same(generated: &ComponentFactoryDescriptor, built: &ComponentFactoryDescriptor) {
    assert_eq!(format!("{generated:?}"), format!("{built:?}"));
}

#[test]
fn every_recipe_attribute_generates_the_builder_descriptor() {
    let replace = factories![replace "replace:carry" => Ledger::carry];
    let transition = factories![transition "transition:pick" => Greeter::pick];
    let hot_update = factories![hot_update "hot_update:relabel" => Banner::relabel];

    assert_same(
        generated::<Ledger>(ComponentFactoryRole::Replacement),
        &replace()[0],
    );
    assert_same(
        generated::<Greeter>(ComponentFactoryRole::Transition),
        &transition()[0],
    );
    assert_same(
        generated::<Banner>(ComponentFactoryRole::HotUpdate),
        &hot_update()[0],
    );
}

#[test]
fn recipe_ids_name_the_marker_and_method() {
    assert_eq!(
        generated::<Ledger>(ComponentFactoryRole::Replacement).id,
        "replace:carry"
    );
    assert_eq!(
        generated::<Greeter>(ComponentFactoryRole::Transition).id,
        "transition:pick"
    );
    assert_eq!(
        generated::<Banner>(ComponentFactoryRole::HotUpdate).id,
        "hot_update:relabel"
    );
}
