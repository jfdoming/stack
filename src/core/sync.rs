use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::{Result, anyhow};

use crate::db::{BranchRecord, Database};
use crate::git::{Git, StashHandle};
use crate::provider::{PrState, Provider};
use crate::views::{OperationView, SyncPlanView};

#[derive(Debug, Clone)]
pub enum SyncOp {
    Fetch {
        remote: String,
    },
    Restack {
        branch: String,
        onto: String,
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
                SyncOp::Fetch { remote } => operations.push(OperationView {
                    kind: "fetch".to_string(),
                    branch: remote.clone(),
                    onto: None,
                    details: format!("fetch {remote}"),
                }),
                SyncOp::Restack {
                    branch,
                    onto,
                    reason,
                } => operations.push(OperationView {
                    kind: "restack".to_string(),
                    branch: branch.clone(),
                    onto: Some(onto.clone()),
                    details: format!("onto {onto}: {reason}"),
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

pub fn build_sync_plan(
    db: &Database,
    git: &Git,
    provider: &dyn Provider,
    base_branch: &str,
    base_remote: &str,
) -> Result<SyncPlan> {
    let tracked = db.list_branches()?;
    let mut ops = vec![SyncOp::Fetch {
        remote: base_remote.to_string(),
    }];
    let mut by_id: HashMap<i64, BranchRecord> = HashMap::new();
    let mut children: HashMap<i64, Vec<i64>> = HashMap::new();

    for b in &tracked {
        by_id.insert(b.id, b.clone());
        if let Some(parent) = b.parent_branch_id {
            children.entry(parent).or_default().push(b.id);
        }
    }

    let mut queue: VecDeque<(String, String)> = VecDeque::new();

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
                    .unwrap_or_else(|| format!("{base_remote}/{base_branch}"));
                if let Some(children_ids) = children.get(&branch.id) {
                    for child_id in children_ids {
                        if let Some(child) = by_id.get(child_id) {
                            queue.push_back((child.name.clone(), new_base.clone()));
                        }
                    }
                }
            }
        }

        let current_sha = git.head_sha(&branch.name)?;
        if let Some(previous_sha) = &branch.last_synced_head_sha
            && previous_sha != &current_sha
            && let Some(children_ids) = children.get(&branch.id)
        {
            for child_id in children_ids {
                if let Some(child) = by_id.get(child_id) {
                    queue.push_back((child.name.clone(), branch.name.clone()));
                }
            }
        }
        ops.push(SyncOp::UpdateSha {
            branch: branch.name.clone(),
            sha: current_sha,
        });
    }

    let mut seen_restack = HashSet::new();
    while let Some((branch, onto)) = queue.pop_front() {
        if !seen_restack.insert(branch.clone()) {
            continue;
        }
        ops.push(SyncOp::Restack {
            branch: branch.clone(),
            onto: onto.clone(),
            reason: "parent updated or merged".to_string(),
        });
        if let Some(node) = tracked.iter().find(|b| b.name == branch)
            && let Some(children_ids) = children.get(&node.id)
        {
            for child_id in children_ids {
                if let Some(child) = by_id.get(child_id) {
                    queue.push_back((child.name.clone(), branch.clone()));
                }
            }
        }
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
                SyncOp::Fetch { remote } => git.fetch_remote(remote)?,
                SyncOp::Restack { branch, onto, .. } => {
                    let old_base = git.merge_base(branch, onto)?;
                    if replay_supported {
                        if let Err(err) = git.replay_onto(branch, &old_base, onto) {
                            let reason = summarize_replay_error(&err);
                            eprintln!(
                                "warning: git replay is unavailable for '{branch}' ({reason}); falling back to rebase"
                            );
                            git.rebase_onto(branch, &old_base, onto)?;
                        }
                    } else {
                        eprintln!("warning: git replay unavailable; using rebase for {branch}");
                        git.rebase_onto(branch, &old_base, onto)?;
                    }
                    let sha = git.head_sha(branch)?;
                    db.set_sync_sha(branch, &sha)?;
                }
                SyncOp::UpdateSha { branch, sha } => db.set_sync_sha(branch, sha)?,
            }
        }
        Ok(())
    })();

    if let Some(stash_handle) = stash
        && let Err(err) = git.stash_pop(&stash_handle)
    {
        eprintln!(
            "warning: could not auto-restore stash {}: {err}",
            stash_handle.reference
        );
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

fn summarize_replay_error(err: &anyhow::Error) -> String {
    let msg = err.to_string();
    if msg.contains("replaying down to root commit is not supported yet") {
        return "cannot replay down to the root commit".to_string();
    }
    if msg.contains("git command failed") {
        return "git replay command failed".to_string();
    }
    msg
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;

    use super::summarize_replay_error;

    #[test]
    fn summarize_replay_error_root_commit_case_is_human_readable() {
        let err = anyhow!(
            "git command failed [\"replay\", \"--onto\", \"main\", \"abc\", \"feat\"]: fatal: replaying down to root commit is not supported yet!"
        );
        let got = summarize_replay_error(&err);
        assert_eq!(got, "cannot replay down to the root commit");
    }

    #[test]
    fn summarize_replay_error_generic_git_failure_is_simplified() {
        let err = anyhow!("git command failed [\"replay\"]: fatal: something broke");
        let got = summarize_replay_error(&err);
        assert_eq!(got, "git replay command failed");
    }
}
