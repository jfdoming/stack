use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::{Result, anyhow};

use crate::db::{BranchRecord, Database};
use crate::git::{Git, StashHandle};
use crate::output::{OperationView, SyncPlanView};
use crate::provider::{PrState, Provider};

#[derive(Debug, Clone)]
pub enum SyncOp {
    Fetch,
    Restack {
        branch: String,
        old_base: String,
        new_base: String,
        reason: String,
    },
    UpdateSha {
        branch: String,
        sha: String,
    },
}

#[derive(Debug, Clone)]
pub struct SyncPlan {
    pub base_branch: String,
    pub ops: Vec<SyncOp>,
}

impl SyncPlan {
    pub fn to_view(&self) -> SyncPlanView {
        let mut operations = Vec::new();
        for op in &self.ops {
            match op {
                SyncOp::Fetch => operations.push(OperationView {
                    kind: "fetch".to_string(),
                    branch: "origin".to_string(),
                    onto: None,
                    details: "fetch origin".to_string(),
                }),
                SyncOp::Restack {
                    branch,
                    old_base,
                    new_base,
                    reason,
                } => operations.push(OperationView {
                    kind: "restack".to_string(),
                    branch: branch.clone(),
                    onto: Some(new_base.clone()),
                    details: format!("from {old_base} to {new_base}: {reason}"),
                }),
                SyncOp::UpdateSha { branch, sha } => operations.push(OperationView {
                    kind: "update_sha".to_string(),
                    branch: branch.clone(),
                    onto: None,
                    details: sha.clone(),
                }),
            }
        }
        SyncPlanView {
            base_branch: self.base_branch.clone(),
            operations,
        }
    }
}

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

pub fn build_sync_plan(
    db: &Database,
    git: &Git,
    provider: &dyn Provider,
    base_branch: &str,
) -> Result<SyncPlan> {
    let tracked = db.list_branches()?;
    let mut ops = vec![SyncOp::Fetch];
    let mut by_id: HashMap<i64, BranchRecord> = HashMap::new();
    let mut children: HashMap<i64, Vec<i64>> = HashMap::new();

    for b in &tracked {
        by_id.insert(b.id, b.clone());
        if let Some(parent) = b.parent_branch_id {
            children.entry(parent).or_default().push(b.id);
        }
    }

    let mut queue: VecDeque<(String, String, String)> = VecDeque::new();

    for branch in &tracked {
        if !git.branch_exists(&branch.name)? {
            continue;
        }

        if let Some(pr) = provider.resolve_pr_by_head(&branch.name, branch.cached_pr_number)? {
            let state = match pr.state {
                PrState::Open => "open",
                PrState::Merged => "merged",
                PrState::Closed => "closed",
                PrState::Unknown => "unknown",
            };
            db.set_pr_cache(&branch.name, Some(pr.number), Some(state))?;

            if matches!(pr.state, PrState::Merged) {
                let new_base = pr
                    .merge_commit_oid
                    .unwrap_or_else(|| format!("origin/{base_branch}"));
                if let Some(children_ids) = children.get(&branch.id) {
                    for child_id in children_ids {
                        if let Some(child) = by_id.get(child_id) {
                            queue.push_back((
                                child.name.clone(),
                                branch.name.clone(),
                                new_base.clone(),
                            ));
                        }
                    }
                }
            }
        }

        let current_sha = git.head_sha(&branch.name)?;
        if let Some(previous_sha) = &branch.last_synced_head_sha {
            if previous_sha != &current_sha {
                if let Some(children_ids) = children.get(&branch.id) {
                    for child_id in children_ids {
                        if let Some(child) = by_id.get(child_id) {
                            queue.push_back((
                                child.name.clone(),
                                branch.name.clone(),
                                current_sha.clone(),
                            ));
                        }
                    }
                }
            }
        }
        ops.push(SyncOp::UpdateSha {
            branch: branch.name.clone(),
            sha: current_sha,
        });
    }

    let mut seen_restack = HashSet::new();
    while let Some((branch, old_base, new_base)) = queue.pop_front() {
        if !seen_restack.insert(branch.clone()) {
            continue;
        }
        ops.push(SyncOp::Restack {
            branch,
            old_base,
            new_base,
            reason: "parent updated or merged".to_string(),
        });
    }

    Ok(SyncPlan {
        base_branch: base_branch.to_string(),
        ops,
    })
}

pub fn execute_sync_plan(db: &Database, git: &Git, plan: &SyncPlan) -> Result<()> {
    let mut stash: Option<StashHandle> = None;
    if git.is_worktree_dirty()? {
        eprintln!("warning: worktree is dirty; auto-stashing local changes");
        stash = git.stash_push("stack-sync-auto-stash")?;
    }

    let run_id = db.record_sync_start()?;
    let mut status = "success";
    let mut summary = None;
    let replay_supported = git.supports_replay();

    let result: Result<()> = (|| {
        for op in &plan.ops {
            match op {
                SyncOp::Fetch => git.fetch_origin()?,
                SyncOp::Restack {
                    branch,
                    old_base,
                    new_base,
                    ..
                } => {
                    if replay_supported {
                        if let Err(err) = git.replay_onto(branch, old_base, new_base) {
                            eprintln!(
                                "warning: git replay failed for {branch}: {err}; falling back to rebase"
                            );
                            git.rebase_onto(branch, old_base, new_base)?;
                        }
                    } else {
                        eprintln!("warning: git replay unavailable; using rebase for {branch}");
                        git.rebase_onto(branch, old_base, new_base)?;
                    }
                    let sha = git.head_sha(branch)?;
                    db.set_sync_sha(branch, &sha)?;
                }
                SyncOp::UpdateSha { branch, sha } => db.set_sync_sha(branch, sha)?,
            }
        }
        Ok(())
    })();

    if let Some(stash_handle) = stash {
        if let Err(err) = git.stash_pop(&stash_handle) {
            eprintln!(
                "warning: could not auto-restore stash {}: {err}",
                stash_handle.reference
            );
        }
    }

    if let Err(err) = result {
        status = "failed";
        summary = Some(format!(
            "{{\"error\":{}}}",
            serde_json::to_string(&err.to_string())?
        ));
        db.record_sync_finish(run_id, status, summary.as_deref())?;
        return Err(anyhow!("sync failed: {err}"));
    }

    db.record_sync_finish(run_id, status, summary.as_deref())?;
    Ok(())
}

pub fn render_plain_tree(branches: &[BranchRecord]) -> String {
    let mut out = String::new();
    let mut children: HashMap<Option<i64>, Vec<&BranchRecord>> = HashMap::new();
    for b in branches {
        children.entry(b.parent_branch_id).or_default().push(b);
    }
    for vals in children.values_mut() {
        vals.sort_by(|a, b| a.name.cmp(&b.name));
    }

    fn walk(
        out: &mut String,
        children: &HashMap<Option<i64>, Vec<&BranchRecord>>,
        parent: Option<i64>,
        prefix: &str,
    ) {
        if let Some(nodes) = children.get(&parent) {
            for (idx, node) in nodes.iter().enumerate() {
                let is_last = idx + 1 == nodes.len();
                let branch_prefix = if is_last { "└──" } else { "├──" };
                out.push_str(&format!("{prefix}{branch_prefix} {}", node.name));
                if let Some(state) = &node.cached_pr_state {
                    out.push_str(&format!(" [pr:{state}]"));
                }
                out.push('\n');
                let next_prefix = if is_last {
                    format!("{prefix}    ")
                } else {
                    format!("{prefix}│   ")
                };
                walk(out, children, Some(node.id), &next_prefix);
            }
        }
    }

    walk(&mut out, &children, None, "");
    if out.is_empty() {
        out.push_str("(no stack branches tracked)\n");
    }
    out
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
