use crate::core::state::SelectedItem;

/// A visible tree node reduced to what navigation needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavNode {
    pub path: Vec<String>,
    /// 0=Collection, 1=Thread, 2=Session, 3=Agent.
    pub category: u8,
    pub has_children: bool,
}

/// Map a resolved selection to a navigation category discriminant.
/// 0=Collection, 1=Thread, 2=Session, 3=Agent. `None` selection -> `None`.
pub fn category_discriminant(item: &SelectedItem) -> Option<u8> {
    match item {
        SelectedItem::None => None,
        SelectedItem::Collection(_) => Some(0),
        SelectedItem::Thread(_, _) => Some(1),
        SelectedItem::Session(_, _, _) => Some(2),
        SelectedItem::Agent(_, _, _, _) => Some(3),
    }
}

/// Next (`forward`) or previous visible node sharing `current`'s category.
/// Visible-only (operates on the given list) and does not wrap. Returns the
/// target path, or `None` if `current` is unknown or there is no further match.
pub fn same_category_target(nodes: &[NavNode], current: &[String], forward: bool) -> Option<Vec<String>> {
    let cur_idx = nodes.iter().position(|n| n.path == current)?;
    let cur_category = nodes[cur_idx].category;

    if forward {
        nodes[cur_idx + 1..]
            .iter()
            .find(|n| n.category == cur_category)
            .map(|n| n.path.clone())
    } else {
        nodes[..cur_idx]
            .iter()
            .rev()
            .find(|n| n.category == cur_category)
            .map(|n| n.path.clone())
    }
}

/// First direct child of `current` in the (already-expanded) visible list.
/// Returns the node immediately following `current` iff it is exactly one
/// segment deeper and prefixed by `current`. `None` for leaves / last node.
pub fn first_child(nodes: &[NavNode], current: &[String]) -> Option<Vec<String>> {
    let cur_idx = nodes.iter().position(|n| n.path == current)?;
    let next = nodes.get(cur_idx + 1)?;
    if next.path.len() == current.len() + 1 && next.path.starts_with(current) {
        Some(next.path.clone())
    } else {
        None
    }
}

/// Parent path (drop the last segment). `None` for top-level or empty paths.
pub fn parent(current: &[String]) -> Option<Vec<String>> {
    if current.len() <= 1 {
        None
    } else {
        Some(current[..current.len() - 1].to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(path: &[&str], category: u8, has_children: bool) -> NavNode {
        NavNode {
            path: path.iter().map(|s| s.to_string()).collect(),
            category,
            has_children,
        }
    }

    fn p(path: &[&str]) -> Vec<String> {
        path.iter().map(|s| s.to_string()).collect()
    }

    fn full_tree() -> Vec<NavNode> {
        vec![
            node(&["col"], 0, true),
            node(&["col", "threadA"], 1, true),
            node(&["col", "threadA", "sessA1"], 2, true),
            node(&["col", "threadA", "sessA1", "agent1"], 3, false),
            node(&["col", "threadA", "sessA2"], 2, false),
            node(&["col", "threadB"], 1, true),
            node(&["col", "threadB", "sessB1"], 2, false),
        ]
    }

    #[test]
    fn next_session_within_same_thread() {
        let nodes = full_tree();
        let target = same_category_target(&nodes, &p(&["col", "threadA", "sessA1"]), true);
        assert_eq!(target, Some(p(&["col", "threadA", "sessA2"])));
    }

    #[test]
    fn next_session_spans_into_next_thread() {
        let nodes = full_tree();
        let target = same_category_target(&nodes, &p(&["col", "threadA", "sessA2"]), true);
        assert_eq!(target, Some(p(&["col", "threadB", "sessB1"])));
    }

    #[test]
    fn next_thread_skips_over_sessions_and_agents() {
        let nodes = full_tree();
        let target = same_category_target(&nodes, &p(&["col", "threadA"]), true);
        assert_eq!(target, Some(p(&["col", "threadB"])));
    }

    #[test]
    fn previous_session_moves_backward() {
        let nodes = full_tree();
        let target = same_category_target(&nodes, &p(&["col", "threadB", "sessB1"]), false);
        assert_eq!(target, Some(p(&["col", "threadA", "sessA2"])));
    }

    #[test]
    fn no_wrap_at_last_of_category() {
        let nodes = full_tree();
        let target = same_category_target(&nodes, &p(&["col", "threadB", "sessB1"]), true);
        assert_eq!(target, None);
    }

    #[test]
    fn no_wrap_at_first_of_category() {
        let nodes = full_tree();
        let target = same_category_target(&nodes, &p(&["col", "threadA", "sessA1"]), false);
        assert_eq!(target, None);
    }

    #[test]
    fn collapsed_children_are_absent_so_skipped() {
        let nodes = vec![
            node(&["col"], 0, true),
            node(&["col", "threadA"], 1, true),
            node(&["col", "threadA", "sessA1"], 2, false),
            node(&["col", "threadB"], 1, true),
        ];
        let target = same_category_target(&nodes, &p(&["col", "threadA", "sessA1"]), true);
        assert_eq!(target, None);
    }

    #[test]
    fn target_for_unknown_current_path_is_none() {
        let nodes = full_tree();
        let target = same_category_target(&nodes, &p(&["nope"]), true);
        assert_eq!(target, None);
    }

    #[test]
    fn first_child_returns_direct_child() {
        let nodes = full_tree();
        let child = first_child(&nodes, &p(&["col", "threadA"]));
        assert_eq!(child, Some(p(&["col", "threadA", "sessA1"])));
    }

    #[test]
    fn first_child_of_leaf_is_none() {
        let nodes = full_tree();
        let child = first_child(&nodes, &p(&["col", "threadA", "sessA2"]));
        assert_eq!(child, None);
    }

    #[test]
    fn first_child_skips_non_descendant() {
        let nodes = full_tree();
        let child = first_child(&nodes, &p(&["col", "threadB", "sessB1"]));
        assert_eq!(child, None);
    }

    #[test]
    fn parent_drops_last_segment() {
        assert_eq!(
            parent(&p(&["col", "threadA", "sessA1"])),
            Some(p(&["col", "threadA"]))
        );
    }

    #[test]
    fn parent_of_top_level_is_none() {
        assert_eq!(parent(&p(&["col"])), None);
        assert_eq!(parent(&[]), None);
    }

    #[test]
    fn discriminant_maps_each_variant() {
        assert_eq!(category_discriminant(&SelectedItem::Collection(0)), Some(0));
        assert_eq!(category_discriminant(&SelectedItem::Thread(0, 0)), Some(1));
        assert_eq!(category_discriminant(&SelectedItem::Session(0, 0, 0)), Some(2));
        assert_eq!(category_discriminant(&SelectedItem::Agent(0, 0, 0, 0)), Some(3));
        assert_eq!(category_discriminant(&SelectedItem::None), None);
    }
}
