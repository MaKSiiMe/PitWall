//! Playlist edits with the calls the server offers.
//!
//! XML-RPC has no "move" or "insert at": only `AddMapList` (append), `RemoveMapList` and
//! `InsertMapList` (after the current map). A new order is applied by keeping the common prefix,
//! removing the rest and appending the new tail. The playlist is never emptied on the way (the
//! server would have nothing to load).

/// Calls that turn one playlist into another.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlaylistPlan {
    /// Added first, so the playlist keeps a map while the others are removed.
    pub add_first: Option<String>,
    pub remove: Vec<String>,
    pub append: Vec<String>,
}

impl PlaylistPlan {
    pub fn is_empty(&self) -> bool {
        self.add_first.is_none() && self.remove.is_empty() && self.append.is_empty()
    }
}

/// Plans the change from `current` to `target` (non-empty, without duplicates).
pub fn plan(current: &[String], target: &[String]) -> PlaylistPlan {
    let Some(first) = target.first() else {
        return PlaylistPlan::default();
    };
    let common = current
        .iter()
        .zip(target)
        .take_while(|(a, b)| a == b)
        .count();
    if common > 0 {
        return PlaylistPlan {
            add_first: None,
            remove: current[common..].to_vec(),
            append: target[common..].to_vec(),
        };
    }
    // Different first map: keep only it, then append the rest.
    PlaylistPlan {
        add_first: (!current.contains(first)).then(|| first.clone()),
        remove: current.iter().filter(|f| *f != first).cloned().collect(),
        append: target[1..].to_vec(),
    }
}

/// Applies a plan to a list the way the server does (used by tests and mocks).
pub fn apply(current: &[String], plan: &PlaylistPlan) -> Vec<String> {
    let mut list = current.to_vec();
    list.extend(plan.add_first.iter().cloned());
    list.retain(|f| !plan.remove.contains(f));
    list.extend(plan.append.iter().cloned());
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    fn check(current: &[&str], target: &[&str]) -> PlaylistPlan {
        let (current, target) = (list(current), list(target));
        let plan = plan(&current, &target);
        assert_eq!(apply(&current, &plan), target, "{plan:?}");
        plan
    }

    #[test]
    fn same_list_does_nothing() {
        assert!(check(&["a", "b", "c"], &["a", "b", "c"]).is_empty());
    }

    #[test]
    fn moves_keep_the_common_prefix() {
        let plan = check(&["a", "b", "c", "d"], &["a", "b", "d", "c"]);
        assert_eq!(plan.remove, list(&["c", "d"]));
        assert_eq!(plan.append, list(&["d", "c"]));
    }

    #[test]
    fn new_first_map_never_empties_the_playlist() {
        let plan = check(&["a", "b", "c"], &["c", "a", "b"]);
        assert_eq!(plan.add_first, None);
        assert_eq!(plan.remove, list(&["a", "b"]));
        let plan = check(&["a", "b"], &["x", "a"]);
        assert_eq!(plan.add_first.as_deref(), Some("x"));
    }

    #[test]
    fn clear_keeps_one_map() {
        let plan = check(&["a", "b", "c"], &["b"]);
        assert_eq!(plan.remove, list(&["a", "c"]));
        assert!(plan.append.is_empty());
    }

    #[test]
    fn shrink_and_grow() {
        check(&["a", "b", "c"], &["a"]);
        check(&["a"], &["a", "b", "c"]);
    }
}
