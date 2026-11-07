//! Walking a hierarchy of Groups or Services through their parents.
//!
//! Nothing in the schema stops an entity from being its own ancestor (ADR
//! 0003), and `cli/si` doesn't either (#2), so every walk here tolerates
//! cycles already in the data, while lager itself never creates one.

use std::collections::{HashMap, HashSet};

/// An entity in a hierarchy: a Group or a Service.
pub(super) trait Node: Clone {
    fn id(&self) -> i64;
    fn name(&self) -> &str;
    fn parent_id(&self) -> Option<i64>;
}

/// Every node in hierarchy order, with its depth: each top-level node
/// followed by its descendants, depth first, siblings by name. A parent
/// that doesn't exist counts as none. Nodes caught in a cycle come last,
/// each listed once.
pub(super) fn tree_order<T: Node>(nodes: &[T]) -> Vec<(usize, T)> {
    let ids: HashSet<i64> = nodes.iter().map(Node::id).collect();
    let mut children: HashMap<Option<i64>, Vec<&T>> = HashMap::new();
    for node in nodes {
        let parent = node.parent_id().filter(|p| ids.contains(p));
        children.entry(parent).or_default().push(node);
    }
    for siblings in children.values_mut() {
        siblings.sort_by(|a, b| a.name().cmp(b.name()));
    }
    let mut tree = Vec::new();
    let mut visited = HashSet::new();
    let mut stack: Vec<(usize, &T)> = children
        .get(&None)
        .map(|roots| roots.iter().rev().map(|n| (0, *n)).collect())
        .unwrap_or_default();
    let mut leftovers: Vec<&T> = nodes.iter().collect();
    leftovers.sort_by(|a, b| b.name().cmp(a.name()));
    loop {
        while let Some((depth, node)) = stack.pop() {
            if !visited.insert(node.id()) {
                continue;
            }
            tree.push((depth, node.clone()));
            if let Some(kids) = children.get(&Some(node.id())) {
                stack.extend(kids.iter().rev().map(|k| (depth + 1, *k)));
            }
        }
        // Whatever is left is unreachable from a top-level node: a cycle.
        match leftovers.pop() {
            Some(node) if !visited.contains(&node.id()) => stack.push((0, node)),
            Some(_) => {}
            None => break,
        }
    }
    tree
}

/// If making `parent_id` the parent of `id` would create a cycle, the
/// loop's names, starting and ending with `id`'s. Walks up from
/// `parent_id`, stopping at any cycle already in the data.
pub(super) fn cycle_through<T: Node>(
    nodes: &HashMap<i64, T>,
    id: i64,
    parent_id: i64,
) -> Option<Vec<String>> {
    let mut path = vec![nodes.get(&id)?.name().to_string()];
    let mut visited = HashSet::new();
    let mut current = Some(parent_id);
    while let Some(at) = current {
        let node = nodes.get(&at)?;
        path.push(node.name().to_string());
        if at == id {
            return Some(path);
        }
        if !visited.insert(at) {
            return None;
        }
        current = node.parent_id();
    }
    None
}

/// The ancestors of `id`, nearest first, each once, stopping at any cycle
/// already in the data and at a parent that doesn't exist.
pub(super) fn ancestors<T: Node>(nodes: &HashMap<i64, T>, id: i64) -> Vec<i64> {
    let mut seen = HashSet::from([id]);
    let mut chain = Vec::new();
    let mut current = nodes.get(&id).and_then(Node::parent_id);
    while let Some(at) = current.filter(|at| nodes.contains_key(at) && seen.insert(*at)) {
        chain.push(at);
        current = nodes[&at].parent_id();
    }
    chain
}

/// The nodes in `start` and all their ancestors, each once, in no particular
/// order. Stops at cycles already in the data, and ignores nodes that don't
/// exist.
pub(super) fn with_ancestors<T: Node>(nodes: &HashMap<i64, T>, start: Vec<i64>) -> Vec<i64> {
    let mut seen = HashSet::new();
    let mut pending = start;
    while let Some(id) = pending.pop() {
        if nodes.contains_key(&id) && seen.insert(id) {
            pending.extend(nodes[&id].parent_id());
        }
    }
    seen.into_iter().collect()
}
