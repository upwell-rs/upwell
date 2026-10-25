//! The `contribute!` macro: declarative, provenance-carrying plugin contributions.

/// Emits explicit app-neutral plugin contributions with stable namespaced identities.
///
/// The macro takes an explicit mutable [`PluginContributions`](crate::PluginContributions)
/// target followed by any supported sections in the order shown below. Every left-hand string
/// is a stable, namespaced [`ContributionId`](crate::ContributionId), not the ID of the
/// component, provider or entry payload.
///
/// ```text
/// contribute! {
///     to <collector expression>,
///     components: [
///         "<contribution id>" => type <component type>,
///         "<contribution id>" => <ComponentDescriptor expression>,
///     ],
///     providers: [
///         "<contribution id>" => <ProviderDescriptor expression>,
///     ],
///     configs: [
///         "<contribution id>" => <ConfigProperties type> => <config path expression>,
///     ],
///     entries: [
///         "<contribution id>" => <EntryDescriptor expression>,
///     ],
/// }
/// ```
///
/// Each section is optional, but sections that are present must follow that order. Entries and
/// sections may have trailing commas.
///
/// In `components`, `type T` is the preferred form and requires `T` to implement
/// `Descriptor<ComponentDescriptor>`. An expression is the explicit escape hatch for a dynamically
/// assembled descriptor or a component that cannot implement `Descriptor`. The `type` marker is
/// required because a bare path can be valid in both Rust's type and expression grammars, while
/// `type T` is unambiguously a typed contribution in this macro.
///
/// ```
/// # use upwell_app::{PluginContributions, contribute};
/// # use upwell_config::ConfigProperties;
/// # use upwell_core::Descriptor;
/// # use upwell_di::{ComponentDescriptor, ProviderDescriptor};
/// # struct Scheduler;
/// # #[derive(serde::Deserialize)]
/// # struct JobsConfig;
/// # impl ConfigProperties for JobsConfig { const NAME: &'static str = "JobsConfig"; }
/// # impl Descriptor<ComponentDescriptor> for Scheduler {
/// #     const DESCRIPTOR: ComponentDescriptor = panic!("documentation-only descriptor");
/// # }
/// # fn provider_descriptor() -> ProviderDescriptor { unimplemented!() }
/// # fn add(contributions: &mut PluginContributions) {
/// contribute! {
///     to contributions,
///     components: [
///         "jobs/scheduler-component" => type Scheduler,
///     ],
///     providers: [
///         "jobs/scheduler-provider" => provider_descriptor(),
///     ],
///     configs: [
///         "jobs/config" => JobsConfig => "jobs",
///     ],
/// }
/// # }
/// ```
///
/// Contribution IDs remain explicit because they identify plugin emissions and their provenance.
/// They are intentionally not inferred from descriptor IDs: separate plugins may emit the same
/// descriptor, and one plugin may emit multiple independently identified contributions.
#[macro_export]
macro_rules! contribute {
    (
        to $contributions:expr
        $(, components: [$($components:tt)*])?
        $(, providers: [
            $(
                $provider_id:literal => $provider:expr
            ),* $(,)?
        ])?
        $(, configs: [
            $(
                $config_id:literal => $config:ty => $path:expr
            ),* $(,)?
        ])?
        $(, entries: [
            $(
                $entry_id:literal => $entry:expr
            ),* $(,)?
        ])?
        $(,)?
    ) => {{
        let __contributions: &mut $crate::PluginContributions = $contributions;

        $(
            $crate::contribute!(@components __contributions; $($components)*);
        )?

        $(
            $(
                __contributions.provider(
                    $crate::namespaced_id!($crate::ContributionId, $provider_id),
                    $provider,
                );
            )*
        )?

        $(
            $(
                __contributions.config::<$config>(
                    $crate::namespaced_id!($crate::ContributionId, $config_id),
                    $path,
                );
            )*
        )?

        $(
            $(
                __contributions.entry(
                    $crate::namespaced_id!($crate::ContributionId, $entry_id),
                    $entry,
                );
            )*
        )?
    }};

    (@components $contributions:ident;) => {};

    (@components
        $contributions:ident;
        $id:literal => type $component:ty,
        $($remaining:tt)*
    ) => {
        $contributions.component::<$component>(
            $crate::namespaced_id!($crate::ContributionId, $id),
        );
        $crate::contribute!(@components $contributions; $($remaining)*);
    };

    (@components
        $contributions:ident;
        $id:literal => type $component:ty
    ) => {
        $contributions.component::<$component>(
            $crate::namespaced_id!($crate::ContributionId, $id),
        );
    };

    (@components
        $contributions:ident;
        $id:literal => $descriptor:expr,
        $($remaining:tt)*
    ) => {
        $contributions.component_descriptor(
            $crate::namespaced_id!($crate::ContributionId, $id),
            $descriptor,
        );
        $crate::contribute!(@components $contributions; $($remaining)*);
    };

    (@components
        $contributions:ident;
        $id:literal => $descriptor:expr
    ) => {
        $contributions.component_descriptor(
            $crate::namespaced_id!($crate::ContributionId, $id),
            $descriptor,
        );
    };
}
