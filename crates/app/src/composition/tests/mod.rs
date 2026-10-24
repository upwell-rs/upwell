use super::*;

const PROTOCOL: ProtocolId = upwell_core::namespaced_id!(ProtocolId, "test/protocol");

fn plugin_id(value: &'static str) -> PluginId {
    PluginId::new(value).expect("valid test plugin id")
}

fn protocol_id(value: &'static str) -> ProtocolId {
    ProtocolId::new(value).expect("valid test protocol id")
}

fn slot_id(value: &'static str) -> PluginSlotId {
    PluginSlotId::new(value).expect("valid test slot id")
}

fn contribution_id(value: &'static str) -> ContributionId {
    ContributionId::new(value).expect("valid test contribution id")
}

fn early(ordinal: u32) -> InstallationProvenance {
    InstallationProvenance::new(InstallationOrigin::ApplicationDeclaration, ordinal)
}

fn late(ordinal: u32) -> InstallationProvenance {
    InstallationProvenance::new(InstallationOrigin::ApplicationConfiguration, ordinal)
}

fn install(id: &'static str, provenance: InstallationProvenance) -> CompositionDirective {
    CompositionDirective::install(PluginDeclaration::new(plugin_id(id), provenance))
}

fn install_with(
    id: &'static str,
    provenance: InstallationProvenance,
    relations: impl IntoIterator<Item = PluginRelation>,
) -> CompositionDirective {
    CompositionDirective::install(
        PluginDeclaration::new(plugin_id(id), provenance).with_relations(relations),
    )
}

fn relation(kind: RelationKind, target: RelationTarget) -> PluginRelation {
    PluginRelation::new(kind, target)
}

fn plugin_target(id: &'static str) -> RelationTarget {
    RelationTarget::Plugin(plugin_id(id))
}

fn slot_target(id: &'static str) -> RelationTarget {
    RelationTarget::Slot(slot_id(id))
}

fn ids(plan: &[ResolvedPlugin]) -> Vec<PluginId> {
    plan.iter().map(ResolvedPlugin::id).collect()
}

fn permutations<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
    fn visit<T: Clone>(start: usize, items: &mut Vec<T>, output: &mut Vec<Vec<T>>) {
        if start == items.len() {
            output.push(items.clone());

            return;
        }

        for index in start..items.len() {
            items.swap(start, index);
            visit(start + 1, items, output);
            items.swap(start, index);
        }
    }

    let mut items = items.to_vec();
    let mut output = Vec::new();

    visit(0, &mut items, &mut output);

    output
}

mod cycles;
mod determinism;
mod graph;
mod identity;
mod late;
mod slots;
