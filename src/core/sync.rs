use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::{Result, anyhow};

use crate::db::{BranchRecord, Database};
use crate::git::{Git, StashHandle};
use crate::provider::{PrState, Provider};
use crate::util::pr_body::{ManagedBranchRef, managed_pr_section, merge_managed_pr_section};
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
    UpdatePrBody {
        branch: String,
        pr_number: i64,
        body: String,
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
                SyncOp::UpdatePrBody {
                    branch, pr_number, ..
                } => operations.push(OperationView {
                    kind: "update_pr_body".to_string(),
                    branch: branch.clone(),
                    onto: None,
                    details: format!("pr #{pr_number}"),
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
    let mut branch_exists: HashMap<String, bool> = HashMap::new();
    for branch in &tracked {
        branch_exists.insert(branch.name.clone(), git.branch_exists(&branch.name)?);
    }
    let metadata_targets: Vec<(&str, Option<i64>)> = tracked
        .iter()
        .filter(|branch| branch_exists.get(&branch.name).copied().unwrap_or(false))
        .map(|branch| (branch.name.as_str(), branch.cached_pr_number))
        .collect();
    let pr_by_branch = provider.resolve_prs_by_head(&metadata_targets)?;

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
        if !branch_exists.get(&branch.name).copied().unwrap_or(false) {
            continue;
        }

        if let Some(pr) = pr_by_branch.get(&branch.name).cloned() {
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

    let base_url = git
        .remote_web_url(base_remote)?
        .or_else(|| git.remote_web_url("origin").ok().flatten())
        .or_else(|| git.remote_web_url("upstream").ok().flatten());
    if let Some(base_url) = base_url {
        for branch in &tracked {
            let Some(pr) = pr_by_branch.get(&branch.name) else {
                continue;
            };
            if !matches!(pr.state, PrState::Open) {
                continue;
            }

            let parent_ref = branch
                .parent_branch_id
                .and_then(|parent_id| by_id.get(&parent_id))
                .map(|parent| ManagedBranchRef {
                    branch: parent.name.clone(),
                    pr_number: pr_by_branch
                        .get(&parent.name)
                        .map(|p| p.number)
                        .or(parent.cached_pr_number),
                    pr_url: pr_by_branch.get(&parent.name).and_then(|p| p.url.clone()),
                });
            let first_child = children.get(&branch.id).and_then(|ids| {
                ids.iter()
                    .filter_map(|id| by_id.get(id))
                    .map(|child| ManagedBranchRef {
                        branch: child.name.clone(),
                        pr_number: pr_by_branch
                            .get(&child.name)
                            .map(|p| p.number)
                            .or(child.cached_pr_number),
                        pr_url: pr_by_branch.get(&child.name).and_then(|p| p.url.clone()),
                    })
                    .min_by(|a, b| a.branch.cmp(&b.branch))
            });
            let pr_root = pr
                .url
                .as_deref()
                .and_then(repo_root_from_pr_url)
                .unwrap_or(base_url.as_str());
            let managed_section = managed_pr_section(
                pr_root,
                base_branch,
                parent_ref.as_ref(),
                first_child.as_ref(),
            );
            let merged_body = merge_managed_pr_section(pr.body.as_deref(), &managed_section);

            let should_update = pr.body.as_deref().map(str::trim) != Some(merged_body.trim());
            if should_update {
                ops.push(SyncOp::UpdatePrBody {
                    branch: branch.name.clone(),
                    pr_number: pr.number,
                    body: merged_body,
                });
            }
        }
    }

    Ok(SyncPlan {
        base_branch: base_branch.to_string(),
        ops,
    })
}

pub fn execute_sync_plan(
    db: &Database,
    git: &Git,
    provider: &dyn Provider,
    plan: &SyncPlan,
) -> Result<()> {
    let starting_branch = git.current_branch()?;
    let mut stash: Option<StashHandle> = None;
    if git.is_worktree_dirty()? {
        eprintln!("warning: worktree is dirty; auto-stashing local changes");
        stash = git.stash_push("stack-sync-auto-stash")?;
    }

    let run_id = db.record_sync_start()?;
    let mut status = "success";
    let mut summary = None;
    let replay_supported = git.supports_replay();

    let op_result: Result<()> = (|| {
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
                SyncOp::UpdatePrBody {
                    pr_number, body, ..
                } => provider.update_pr_body(*pr_number, body)?,
            }
        }
        Ok(())
    })();

    let restore_branch_result = restore_starting_branch(git, &starting_branch);

    if let Some(stash_handle) = stash
        && let Err(err) = git.stash_pop(&stash_handle)
    {
        eprintln!(
            "warning: could not auto-restore stash {}: {err}",
            stash_handle.reference
        );
    }

    let result = match (op_result, restore_branch_result) {
        (Err(op_err), Err(restore_err)) => Err(anyhow!(
            "{op_err}; additionally failed to restore prior branch '{}': {restore_err}",
            starting_branch
        )),
        (Err(op_err), Ok(())) => Err(op_err),
        (Ok(()), Err(restore_err)) => Err(anyhow!(
            "failed to restore prior branch '{}': {restore_err}",
            starting_branch
        )),
        (Ok(()), Ok(())) => Ok(()),
    };

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

fn repo_root_from_pr_url(url: &str) -> Option<&str> {
    url.split_once("/pull/").map(|(root, _)| root)
}

fn restore_starting_branch(git: &Git, starting_branch: &str) -> Result<()> {
    if starting_branch.is_empty() {
        return Ok(());
    }
    let current_branch = git.current_branch()?;
    if current_branch == starting_branch {
        return Ok(());
    }
    git.checkout_branch(starting_branch)
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
