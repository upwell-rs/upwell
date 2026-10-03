use std::any::TypeId;
use std::collections::{HashMap, HashSet};

/// Tarjan traversal state for deterministic strongly connected components.
struct Search<'a> {
    edges: &'a HashMap<TypeId, HashSet<TypeId>>,
    keys: &'a HashMap<TypeId, String>,
    allowed: HashSet<TypeId>,
    next_index: usize,
    indices: HashMap<TypeId, usize>,
    lowlinks: HashMap<TypeId, usize>,
    stack: Vec<TypeId>,
    on_stack: HashSet<TypeId>,
    cyclic: Vec<Vec<TypeId>>,
}

pub(crate) fn members(
    nodes: &[TypeId],
    edges: &HashMap<TypeId, HashSet<TypeId>>,
    keys: &HashMap<TypeId, String>,
) -> Vec<TypeId> {
    components(nodes, edges, keys)
        .into_iter()
        .flatten()
        .collect()
}

pub(crate) fn components(
    nodes: &[TypeId],
    edges: &HashMap<TypeId, HashSet<TypeId>>,
    keys: &HashMap<TypeId, String>,
) -> Vec<Vec<TypeId>> {
    let mut ordered = nodes.to_vec();
    let mut search = Search {
        edges,
        keys,
        allowed: nodes.iter().copied().collect(),
        next_index: 0,
        indices: HashMap::new(),
        lowlinks: HashMap::new(),
        stack: Vec::new(),
        on_stack: HashSet::new(),
        cyclic: Vec::new(),
    };

    ordered.sort_by(|left, right| keys[left].cmp(&keys[right]));

    for node in ordered {
        if !search.indices.contains_key(&node) {
            search.visit(node);
        }
    }

    for component in &mut search.cyclic {
        component.sort_by(|left, right| keys[left].cmp(&keys[right]));
    }

    search.cyclic.sort_by(|left, right| {
        keys[&left[0]]
            .cmp(&keys[&right[0]])
            .then_with(|| left.len().cmp(&right.len()))
    });

    search.cyclic
}

impl Search<'_> {
    fn visit(&mut self, node: TypeId) {
        let index = self.next_index;

        self.next_index += 1;
        self.indices.insert(node, index);
        self.lowlinks.insert(node, index);
        self.stack.push(node);
        self.on_stack.insert(node);

        let mut successors = self
            .edges
            .get(&node)
            .into_iter()
            .flatten()
            .filter(|successor| self.allowed.contains(successor))
            .copied()
            .collect::<Vec<_>>();

        successors.sort_by(|left, right| self.keys[left].cmp(&self.keys[right]));

        for successor in successors {
            if !self.indices.contains_key(&successor) {
                self.visit(successor);
                self.lower(node, self.lowlinks[&successor]);
            } else if self.on_stack.contains(&successor) {
                self.lower(node, self.indices[&successor]);
            }
        }

        if self.lowlinks[&node] == self.indices[&node] {
            self.finish_component(node);
        }
    }

    fn lower(&mut self, node: TypeId, candidate: usize) {
        let lowlink = self.lowlinks.get_mut(&node).expect("node lowlink exists");

        *lowlink = (*lowlink).min(candidate);
    }

    fn finish_component(&mut self, root: TypeId) {
        let mut component = Vec::new();

        loop {
            let node = self.stack.pop().expect("SCC root remains on the stack");

            self.on_stack.remove(&node);
            component.push(node);

            if node == root {
                break;
            }
        }

        let self_cycle = component.len() == 1
            && self
                .edges
                .get(&component[0])
                .is_some_and(|successors| successors.contains(&component[0]));

        if component.len() > 1 || self_cycle {
            self.cyclic.push(component);
        }
    }
}
