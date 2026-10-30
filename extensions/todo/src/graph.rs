//! Dependency graph checks over `blockedBy` edges.

use std::collections::{HashMap, HashSet};

use crate::model::Task;

/// Whether giving task `id` the dependency list `deps` closes a cycle.
pub fn detect_cycle(tasks: &[Task], id: u64, deps: &[u64]) -> bool {
    let mut edges: HashMap<u64, Vec<u64>> = HashMap::new();
    for task in tasks {
        let mut list = task.blocked_by.clone();
        if task.id == id {
            list.extend(deps.iter().filter(|d| !task.blocked_by.contains(d)));
        }
        edges.insert(task.id, list);
    }
    let mut visited = HashSet::new();
    let mut visiting = HashSet::new();
    let keys: Vec<u64> = edges.keys().copied().collect();
    keys.into_iter()
        .any(|node| cycle_from(node, &edges, &mut visiting, &mut visited))
}

fn cycle_from(
    node: u64,
    edges: &HashMap<u64, Vec<u64>>,
    visiting: &mut HashSet<u64>,
    visited: &mut HashSet<u64>,
) -> bool {
    if visiting.contains(&node) {
        return true;
    }
    if !visited.insert(node) {
        return false;
    }
    visiting.insert(node);
    let found = edges
        .get(&node)
        .into_iter()
        .flatten()
        .any(|next| cycle_from(*next, edges, visiting, visited));
    visiting.remove(&node);
    found
}

/// Reverse edges: for each task, the tasks that list it in `blockedBy`.
pub fn blocks_of(tasks: &[Task], id: u64) -> Vec<u64> {
    tasks
        .iter()
        .filter(|t| t.blocked_by.contains(&id))
        .map(|t| t.id)
        .collect()
}
