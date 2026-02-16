use std::collections::HashSet;

use crate::db::BranchRecord;

pub fn rank_parent_candidates(
    current: &str,
    tracked: &[BranchRecord],
    local: &[String],
) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    push_unique(&mut out, &mut seen, current);

    for b in tracked {
        push_unique(&mut out, &mut seen, &b.name);
    }

    for b in local {
        push_unique(&mut out, &mut seen, b);
    }

    out
}

fn push_unique(out: &mut Vec<String>, seen: &mut HashSet<String>, value: &str) {
    if seen.insert(value.to_string()) {
        out.push(value.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::BranchRecord;

    #[test]
    fn ranking_starts_with_current_then_tracked() {
        let tracked = vec![
            BranchRecord {
                id: 1,
                name: "feat/b".to_string(),
                parent_branch_id: None,
                last_synced_head_sha: None,
                cached_pr_number: None,
                cached_pr_state: None,
            },
            BranchRecord {
                id: 2,
                name: "feat/a".to_string(),
                parent_branch_id: None,
                last_synced_head_sha: None,
                cached_pr_number: None,
                cached_pr_state: None,
            },
        ];
        let local = vec![
            "main".to_string(),
            "feat/a".to_string(),
            "fix/c".to_string(),
        ];
        let ranked = rank_parent_candidates("feat/current", &tracked, &local);
        assert_eq!(ranked[0], "feat/current");
        assert_eq!(ranked[1], "feat/b");
        assert_eq!(ranked[2], "feat/a");
        assert!(ranked.contains(&"fix/c".to_string()));
    }
}
