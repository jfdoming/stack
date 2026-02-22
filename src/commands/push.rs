use anyhow::Result;

use crate::db::Database;
use crate::git::Git;

pub fn run(db: &Database, git: &Git, porcelain: bool, base_branch: &str) -> Result<()> {
    let records = db.list_branches()?;
    let mut branches: Vec<(String, bool)> = records
        .iter()
        .filter(|record| record.name != base_branch)
        .map(|record| {
            let is_merged = record
                .cached_pr_state
                .as_deref()
                .is_some_and(|state| state.eq_ignore_ascii_case("merged"));
            (record.name.clone(), is_merged)
        })
        .collect();
    branches.sort_by(|a, b| a.0.cmp(&b.0));
    branches.dedup();

    let mut pushed = Vec::new();
    let mut skipped_missing = Vec::new();
    let mut skipped_merged = Vec::new();

    for (branch, is_merged) in branches {
        if is_merged {
            skipped_merged.push(branch);
            continue;
        }
        if !git.branch_exists(&branch)? {
            skipped_missing.push(branch);
            continue;
        }

        let remote = git
            .remote_for_branch(&branch)?
            .or_else(|| git.remote_for_branch(base_branch).ok().flatten())
            .unwrap_or_else(|| "origin".to_string());
        git.push_branch_force_with_lease(&remote, &branch)?;
        pushed.push((branch, remote));
    }

    if porcelain {
        let pushed = pushed
            .iter()
            .map(|(branch, remote)| serde_json::json!({ "branch": branch, "remote": remote }))
            .collect::<Vec<_>>();
        return crate::views::print_json(&serde_json::json!({
            "pushed": pushed,
            "skipped_missing": skipped_missing,
            "skipped_merged": skipped_merged,
        }));
    }

    if pushed.is_empty() {
        println!("no tracked non-base branches to push");
    } else {
        for (branch, remote) in &pushed {
            println!("pushed '{branch}' to '{remote}'");
        }
    }

    if !skipped_missing.is_empty() {
        eprintln!(
            "warning: skipped missing tracked branches: {}",
            skipped_missing.join(", ")
        );
    }
    if !skipped_merged.is_empty() {
        eprintln!(
            "warning: skipped merged tracked branches: {}",
            skipped_merged.join(", ")
        );
    }

    Ok(())
}
