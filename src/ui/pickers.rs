use std::collections::HashMap;

use crate::db::BranchRecord;

pub fn build_branch_picker_items(
    ordered_names: &[String],
    current: &str,
    tracked: &[BranchRecord],
) -> Vec<String> {
    let tracked_map: HashMap<&str, &BranchRecord> =
        tracked.iter().map(|b| (b.name.as_str(), b)).collect();
    ordered_names
        .iter()
        .map(|name| {
            if name == current {
                format!("● current  {name}")
            } else if let Some(rec) = tracked_map.get(name.as_str()) {
                let pr = rec.cached_pr_state.as_deref().unwrap_or("none");
                format!("◆ tracked  {name}  (pr:{pr})")
            } else {
                format!("○ local    {name}")
            }
        })
        .collect()
}

pub fn build_delete_picker_items(
    tracked_names: &[String],
    current: &str,
    tracked: &[BranchRecord],
) -> Vec<String> {
    let tracked_map: HashMap<&str, &BranchRecord> =
        tracked.iter().map(|b| (b.name.as_str(), b)).collect();
    tracked_names
        .iter()
        .map(|name| {
            if name == current {
                format!("● current  {name}")
            } else if let Some(rec) = tracked_map.get(name.as_str()) {
                let pr = rec.cached_pr_state.as_deref().unwrap_or("none");
                format!("◆ tracked  {name}  (pr:{pr})")
            } else {
                format!("◆ tracked  {name}")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_picker_items_include_source_labels() {
        let tracked = vec![BranchRecord {
            id: 1,
            name: "feat/a".to_string(),
            parent_branch_id: None,
            last_synced_head_sha: None,
            cached_pr_number: Some(10),
            cached_pr_state: Some("open".to_string()),
        }];
        let ordered = vec![
            "main".to_string(),
            "feat/a".to_string(),
            "fix/local".to_string(),
        ];
        let items = build_branch_picker_items(&ordered, "main", &tracked);
        assert!(items[0].starts_with("● current"));
        assert!(items[1].starts_with("◆ tracked"));
        assert!(items[2].starts_with("○ local"));
    }

    #[test]
    fn delete_picker_items_highlight_current() {
        let tracked = vec![
            BranchRecord {
                id: 1,
                name: "feat/a".to_string(),
                parent_branch_id: None,
                last_synced_head_sha: None,
                cached_pr_number: Some(10),
                cached_pr_state: Some("open".to_string()),
            },
            BranchRecord {
                id: 2,
                name: "feat/b".to_string(),
                parent_branch_id: None,
                last_synced_head_sha: None,
                cached_pr_number: None,
                cached_pr_state: None,
            },
        ];
        let names = vec!["feat/a".to_string(), "feat/b".to_string()];
        let items = build_delete_picker_items(&names, "feat/b", &tracked);
        assert!(items[0].starts_with("◆ tracked"));
        assert!(items[1].starts_with("● current"));
    }
}
