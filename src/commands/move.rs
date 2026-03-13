use std::collections::{HashMap, HashSet};
use std::io::{IsTerminal, stdin, stdout};

use anyhow::{Result, anyhow};
use dialoguer::{Select, theme::ColorfulTheme};

use crate::args::MoveArgs;
use crate::core::{build_sync_plan, execute_sync_plan};
use crate::db::{BranchRecord, Database};
use crate::git::Git;
use crate::provider::Provider;
use crate::ui::interaction::prompt_or_cancel;
use crate::ui::pickers::build_branch_picker_items;

pub fn run(
    db: &Database,
    git: &Git,
    provider: &dyn Provider,
    args: &MoveArgs,
    porcelain: bool,
    base_branch: &str,
    base_remote: &str,
) -> Result<()> {
    let is_tty = stdout().is_terminal() && stdin().is_terminal();
    let current = git.current_branch()?;
    let tracked = db.list_branches()?;
    let tracked_by_name: HashMap<String, BranchRecord> = tracked
        .iter()
        .map(|branch| (branch.name.clone(), branch.clone()))
        .collect();
    let local = git.local_branches()?;

    let target = resolve_target(
        args,
        is_tty,
        porcelain,
        &current,
        base_branch,
        &local,
        &tracked,
    )?;
    let descendants = descendants_of(&target, &tracked);
    let parent = resolve_parent(
        args,
        is_tty,
        &current,
        &target,
        &descendants,
        &tracked,
        &tracked_by_name,
        &local,
    )?;

    if parent == target {
        return Err(anyhow!("target and parent branch names are identical"));
    }

    db.set_parent(&target, Some(&parent))?;
    let (sync_plan, _) = build_sync_plan(db, git, provider, base_branch, base_remote)?;
    let sync_view = sync_plan.to_view();
    if !sync_view.operations.is_empty() {
        execute_sync_plan(db, git, provider, &sync_plan)?;
    }

    let previous_parent = tracked_by_name
        .get(&target)
        .and_then(|branch| branch.parent_branch_id)
        .and_then(|parent_id| {
            tracked
                .iter()
                .find(|branch| branch.id == parent_id)
                .map(|branch| branch.name.clone())
        });

    let payload = serde_json::json!({
        "action": "move",
        "target": target,
        "old_parent": previous_parent,
        "new_parent": parent,
        "applied": true,
        "sync": {
            "applied": !sync_view.operations.is_empty(),
            "operations": sync_view.operations,
        }
    });

    if porcelain {
        return crate::views::print_json(&payload);
    }

    println!(
        "moved '{}' and its descendants under '{}'",
        payload["target"].as_str().unwrap_or_default(),
        payload["new_parent"].as_str().unwrap_or_default()
    );
    if payload["sync"]["applied"].as_bool().unwrap_or(false) {
        println!("sync completed");
    } else {
        println!("sync already up to date");
    }
    Ok(())
}

fn resolve_target(
    args: &MoveArgs,
    is_tty: bool,
    porcelain: bool,
    current: &str,
    base_branch: &str,
    local: &[String],
    tracked: &[BranchRecord],
) -> Result<String> {
    if let Some(target) = &args.target {
        return ensure_target_candidate(target.trim(), base_branch, local);
    }

    let candidates = target_candidates(local, base_branch);
    if candidates.is_empty() {
        return Err(anyhow!("no local non-base branches available to move"));
    }

    let default_idx = target_default_index(&candidates, current);

    if !is_tty {
        if candidates[default_idx] == current {
            if !porcelain {
                println!("defaulting target branch to current branch '{current}'");
            }
            return Ok(current.to_string());
        }
        return Err(anyhow!(
            "current branch '{}' cannot be used as move target; pass stack move <target> --parent <parent>",
            current
        ));
    }

    let theme = ColorfulTheme::default();
    let picker_items = build_branch_picker_items(&candidates, current, tracked);
    let idx = prompt_or_cancel(
        Select::with_theme(&theme)
            .with_prompt(
                "Select branch to move (↑/↓ to navigate, Enter to select, Ctrl-C to cancel)",
            )
            .items(&picker_items)
            .default(default_idx)
            .interact(),
    )?;

    Ok(candidates[idx].clone())
}

fn resolve_parent(
    args: &MoveArgs,
    is_tty: bool,
    current: &str,
    target: &str,
    descendants: &HashSet<String>,
    tracked: &[BranchRecord],
    tracked_by_name: &HashMap<String, BranchRecord>,
    local: &[String],
) -> Result<String> {
    if let Some(parent) = &args.parent {
        return ensure_parent_candidate(parent.trim(), target, descendants, tracked_by_name, local);
    }

    if !is_tty {
        return Err(anyhow!(
            "missing parent branch; usage: stack move [target] --parent <parent>"
        ));
    }

    let candidates = parent_candidates(target, descendants, local);
    if candidates.is_empty() {
        return Err(anyhow!(
            "no viable parent branches available for '{}'",
            target
        ));
    }

    let theme = ColorfulTheme::default();
    let picker_items = build_branch_picker_items(&candidates, current, tracked);
    let default_idx = candidates
        .iter()
        .position(|name| name == current)
        .unwrap_or(0);
    let idx = prompt_or_cancel(
        Select::with_theme(&theme)
            .with_prompt(format!(
                "Select new parent branch for '{}' (↑/↓ to navigate, Enter to select, Ctrl-C to cancel)",
                target
            ))
            .items(&picker_items)
            .default(default_idx)
            .interact(),
    )?;

    Ok(candidates[idx].clone())
}

fn ensure_target_candidate(target: &str, base_branch: &str, local: &[String]) -> Result<String> {
    if target.is_empty() {
        return Err(anyhow!("target branch name cannot be empty"));
    }
    if target == base_branch {
        return Err(anyhow!("cannot move base branch '{base_branch}'"));
    }
    if !local.iter().any(|branch| branch == target) {
        return Err(anyhow!("branch does not exist in git: {}", target));
    }
    Ok(target.to_string())
}

fn target_candidates(local: &[String], base_branch: &str) -> Vec<String> {
    local
        .iter()
        .filter(|branch| branch.as_str() != base_branch)
        .cloned()
        .collect()
}

fn target_default_index(candidates: &[String], current: &str) -> usize {
    candidates
        .iter()
        .position(|name| name == current)
        .unwrap_or(0)
}

fn ensure_parent_candidate(
    parent: &str,
    target: &str,
    descendants: &HashSet<String>,
    tracked_by_name: &HashMap<String, BranchRecord>,
    local: &[String],
) -> Result<String> {
    if parent.is_empty() {
        return Err(anyhow!("parent branch name cannot be empty"));
    }
    if parent == target {
        return Err(anyhow!("target and parent branch names are identical"));
    }
    if descendants.contains(parent) {
        return Err(anyhow!("link would create a cycle"));
    }
    if !tracked_by_name.contains_key(parent) && !local.iter().any(|branch| branch == parent) {
        return Err(anyhow!("parent branch does not exist in git: {}", parent));
    }
    Ok(parent.to_string())
}

fn parent_candidates(target: &str, descendants: &HashSet<String>, local: &[String]) -> Vec<String> {
    local
        .iter()
        .filter(|branch| branch.as_str() != target && !descendants.contains(branch.as_str()))
        .cloned()
        .collect()
}

fn descendants_of(target: &str, tracked: &[BranchRecord]) -> HashSet<String> {
    let mut children_by_parent: HashMap<i64, Vec<&BranchRecord>> = HashMap::new();
    let by_name: HashMap<&str, &BranchRecord> = tracked
        .iter()
        .map(|branch| (branch.name.as_str(), branch))
        .collect();

    for branch in tracked {
        if let Some(parent_id) = branch.parent_branch_id {
            children_by_parent
                .entry(parent_id)
                .or_default()
                .push(branch);
        }
    }

    let mut descendants = HashSet::new();
    let Some(root) = by_name.get(target) else {
        return descendants;
    };
    let mut stack: Vec<i64> = vec![root.id];
    while let Some(parent_id) = stack.pop() {
        if let Some(children) = children_by_parent.get(&parent_id) {
            for child in children {
                if descendants.insert(child.name.clone()) {
                    stack.push(child.id);
                }
            }
        }
    }
    descendants
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_default_index_prefers_current_branch_when_present() {
        let candidates = vec![
            "feat/a".to_string(),
            "feat/current".to_string(),
            "feat/b".to_string(),
        ];
        assert_eq!(target_default_index(&candidates, "feat/current"), 1);
    }

    #[test]
    fn target_default_index_falls_back_to_first_candidate() {
        let candidates = vec!["feat/a".to_string(), "feat/b".to_string()];
        assert_eq!(target_default_index(&candidates, "feat/missing"), 0);
    }

    #[test]
    fn target_candidates_exclude_base_branch() {
        let local = vec![
            "main".to_string(),
            "feat/a".to_string(),
            "feat/b".to_string(),
        ];
        assert_eq!(
            target_candidates(&local, "main"),
            vec!["feat/a".to_string(), "feat/b".to_string()]
        );
    }
}
