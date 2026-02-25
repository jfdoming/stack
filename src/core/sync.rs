use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

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
    UpdateBaseToMergeCommit {
        branch: String,
        merge_commit: String,
    },
    Restack {
        branch: String,
        onto: String,
        old_base: Option<String>,
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
    PruneMergedBranch {
        branch: String,
    },
}

#[derive(Debug, Clone)]
pub struct SyncPlan {
    pub base_branch: String,
    pub ops: Vec<SyncOp>,
}

#[derive(Debug, Clone)]
pub struct SyncPlanTiming {
    pub setup: Duration,
    pub pr_lookup: Duration,
    pub assemble: Duration,
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
                SyncOp::UpdateBaseToMergeCommit {
                    branch,
                    merge_commit,
                } => operations.push(OperationView {
                    kind: "update_base".to_string(),
                    branch: branch.clone(),
                    onto: Some(merge_commit.clone()),
                    details: format!("ff-only to merged commit {merge_commit}"),
                }),
                SyncOp::Restack {
                    branch,
                    onto,
                    old_base: _,
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
                SyncOp::PruneMergedBranch { branch } => operations.push(OperationView {
                    kind: "prune_merged".to_string(),
                    branch: branch.clone(),
                    onto: None,
                    details: "remove merged branch from local refs and stack metadata".to_string(),
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
) -> Result<(SyncPlan, SyncPlanTiming)> {
    #[derive(Clone)]
    struct RestackCandidate {
        branch: String,
        onto: String,
        old_base: Option<String>,
    }

    let setup_started = Instant::now();
    let sync_remote = git.preferred_sync_remote(base_remote)?;
    let tracked = db.list_branches()?;
    let mut branch_exists: HashMap<String, bool> = HashMap::new();
    for branch in &tracked {
        branch_exists.insert(branch.name.clone(), git.branch_exists(&branch.name)?);
    }
    let setup_elapsed = setup_started.elapsed();

    let pr_lookup_started = Instant::now();
    let metadata_targets: Vec<(&str, Option<i64>)> = tracked
        .iter()
        .filter(|branch| branch.name != base_branch)
        .map(|branch| (branch.name.as_str(), branch.cached_pr_number))
        .collect();
    let pr_by_branch = provider.resolve_prs_by_head(&metadata_targets)?;
    let pr_lookup_elapsed = pr_lookup_started.elapsed();

    let assemble_started = Instant::now();

    let mut ops = Vec::new();
    let mut needs_fetch = false;
    let mut current_sha_by_branch: HashMap<String, String> = HashMap::new();
    let mut by_id: HashMap<i64, BranchRecord> = HashMap::new();
    let mut children: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut base_merge_commit_to_apply: Option<String> = None;
    let mut merged_state_by_branch: HashMap<String, bool> = HashMap::new();

    for b in &tracked {
        by_id.insert(b.id, b.clone());
        if let Some(parent) = b.parent_branch_id {
            children.entry(parent).or_default().push(b.id);
        }
    }

    let mut queue: VecDeque<RestackCandidate> = VecDeque::new();

    for branch in &tracked {
        let branch_is_local = branch_exists.get(&branch.name).copied().unwrap_or(false);
        let current_sha = if branch_is_local {
            let sha = git.head_sha(&branch.name)?;
            current_sha_by_branch.insert(branch.name.clone(), sha.clone());
            Some(sha)
        } else {
            None
        };
        let mut merged_restack_base: Option<String> = None;

        let mut is_merged_pr = branch
            .cached_pr_state
            .as_deref()
            .is_some_and(|state| state.eq_ignore_ascii_case("merged"));
        if let Some(pr) = pr_by_branch.get(&branch.name).cloned() {
            let state = match pr.state {
                PrState::Open => "open",
                PrState::Merged => "merged",
                PrState::Closed => "closed",
                PrState::Unknown => "unknown",
            };
            db.set_pr_cache(&branch.name, Some(pr.number), Some(state))?;
            is_merged_pr = matches!(pr.state, PrState::Merged);

            if matches!(pr.state, PrState::Merged) {
                let merge_commit_oid = pr.merge_commit_oid.clone();
                merged_restack_base = Some(
                    merge_commit_oid
                        .clone()
                        .unwrap_or_else(|| format!("{sync_remote}/{base_branch}")),
                );

                let is_direct_child_of_base = branch
                    .parent_branch_id
                    .and_then(|parent_id| by_id.get(&parent_id))
                    .is_some_and(|parent| parent.name == base_branch);
                if is_direct_child_of_base
                    && base_merge_commit_to_apply.is_none()
                    && let Some(merge_commit_oid) = merge_commit_oid.as_deref()
                {
                    base_merge_commit_to_apply = Some(merge_commit_oid.to_string());
                }
            }
        }
        merged_state_by_branch.insert(branch.name.clone(), is_merged_pr);

        if !branch_is_local {
            continue;
        }
        if branch.name == base_branch {
            db.set_pr_cache(&branch.name, None, None)?;
        }
        let current_sha = current_sha.expect("local branch SHA should be available");

        if is_merged_pr {
            let new_base =
                merged_restack_base.unwrap_or_else(|| format!("{sync_remote}/{base_branch}"));
            if let Some(children_ids) = children.get(&branch.id) {
                for child_id in children_ids {
                    if let Some(child) = by_id.get(child_id) {
                        if !branch_exists.get(&child.name).copied().unwrap_or(false) {
                            continue;
                        }
                        let should_restack = if git.ref_exists(&new_base)? {
                            !git.is_ancestor(&new_base, &child.name)?
                        } else {
                            true
                        };
                        if should_restack {
                            let child_merged = pr_by_branch
                                .get(&child.name)
                                .map(|pr| matches!(pr.state, PrState::Merged))
                                .unwrap_or_else(|| {
                                    child
                                        .cached_pr_state
                                        .as_deref()
                                        .is_some_and(|state| state.eq_ignore_ascii_case("merged"))
                                });
                            if child_merged {
                                continue;
                            }
                            needs_fetch = true;
                            queue.push_back(RestackCandidate {
                                branch: child.name.clone(),
                                onto: new_base.clone(),
                                old_base: Some(current_sha.clone()),
                            });
                        }
                    }
                }
            }
        }

        if !is_merged_pr {
            if let Some(parent_id) = branch.parent_branch_id
                && let Some(parent) = by_id.get(&parent_id)
                && branch_exists.get(&parent.name).copied().unwrap_or(false)
            {
                let parent_is_merged = pr_by_branch
                    .get(&parent.name)
                    .map(|pr| matches!(pr.state, PrState::Merged))
                    .unwrap_or_else(|| {
                        parent
                            .cached_pr_state
                            .as_deref()
                            .is_some_and(|state| state.eq_ignore_ascii_case("merged"))
                    });
                if !parent_is_merged {
                    let parent_onto = if parent.name == base_branch {
                        let remote_base_ref = format!("{sync_remote}/{base_branch}");
                        if git.ref_exists(&remote_base_ref)? {
                            remote_base_ref
                        } else {
                            parent.name.clone()
                        }
                    } else {
                        parent.name.clone()
                    };
                    if !git.is_ancestor(&parent_onto, &branch.name)? {
                        queue.push_back(RestackCandidate {
                            branch: branch.name.clone(),
                            onto: parent.name.clone(),
                            old_base: None,
                        });
                    }
                }
            }
            if let Some(previous_sha) = &branch.last_synced_head_sha
                && previous_sha != &current_sha
                && let Some(children_ids) = children.get(&branch.id)
            {
                for child_id in children_ids {
                    if let Some(child) = by_id.get(child_id) {
                        if !branch_exists.get(&child.name).copied().unwrap_or(false) {
                            continue;
                        }
                        let child_merged = pr_by_branch
                            .get(&child.name)
                            .map(|pr| matches!(pr.state, PrState::Merged))
                            .unwrap_or_else(|| {
                                child
                                    .cached_pr_state
                                    .as_deref()
                                    .is_some_and(|state| state.eq_ignore_ascii_case("merged"))
                            });
                        if child_merged {
                            continue;
                        }
                        queue.push_back(RestackCandidate {
                            branch: child.name.clone(),
                            onto: branch.name.clone(),
                            old_base: None,
                        });
                    }
                }
            }
        }
        let has_base_merge_update = base_merge_commit_to_apply
            .as_deref()
            .is_some_and(|merge_commit| branch.name == base_branch && merge_commit != current_sha);
        if !is_merged_pr
            && !has_base_merge_update
            && branch.last_synced_head_sha.as_deref() != Some(current_sha.as_str())
        {
            ops.push(SyncOp::UpdateSha {
                branch: branch.name.clone(),
                sha: current_sha,
            });
        }
    }

    if let Some(merge_commit) = base_merge_commit_to_apply {
        let base_current_sha = current_sha_by_branch.get(base_branch).cloned();
        let base_already_contains_merge = if let Some(base_sha) = base_current_sha.as_deref() {
            if base_sha == merge_commit {
                true
            } else if git.ref_exists(&merge_commit)? {
                git.is_ancestor(&merge_commit, base_branch)?
            } else {
                false
            }
        } else {
            false
        };
        if !base_already_contains_merge {
            needs_fetch = true;
            ops.insert(
                0,
                SyncOp::UpdateBaseToMergeCommit {
                    branch: base_branch.to_string(),
                    merge_commit,
                },
            );
        }
    }

    let mut seen_restack = HashSet::new();
    while let Some(item) = queue.pop_front() {
        if !branch_exists.get(&item.branch).copied().unwrap_or(false) {
            continue;
        }
        if merged_state_by_branch
            .get(&item.branch)
            .copied()
            .unwrap_or(false)
        {
            continue;
        }
        if !seen_restack.insert(item.branch.clone()) {
            continue;
        }
        needs_fetch = true;
        ops.push(SyncOp::Restack {
            branch: item.branch.clone(),
            onto: item.onto.clone(),
            old_base: item
                .old_base
                .or_else(|| current_sha_by_branch.get(&item.onto).cloned()),
            reason: "parent updated or merged".to_string(),
        });
        if let Some(node) = tracked.iter().find(|b| b.name == item.branch)
            && let Some(children_ids) = children.get(&node.id)
        {
            for child_id in children_ids {
                if let Some(child) = by_id.get(child_id) {
                    if !branch_exists.get(&child.name).copied().unwrap_or(false) {
                        continue;
                    }
                    if merged_state_by_branch
                        .get(&child.name)
                        .copied()
                        .unwrap_or_else(|| {
                            child
                                .cached_pr_state
                                .as_deref()
                                .is_some_and(|state| state.eq_ignore_ascii_case("merged"))
                        })
                    {
                        continue;
                    }
                    queue.push_back(RestackCandidate {
                        branch: child.name.clone(),
                        onto: item.branch.clone(),
                        old_base: None,
                    });
                }
            }
        }
    }

    let stack_fully_merged = tracked
        .iter()
        .filter(|branch| branch.name != base_branch)
        .all(|branch| {
            merged_state_by_branch
                .get(&branch.name)
                .copied()
                .unwrap_or_else(|| {
                    branch
                        .cached_pr_state
                        .as_deref()
                        .is_some_and(|state| state.eq_ignore_ascii_case("merged"))
                })
        });
    if stack_fully_merged {
        let mut prune_candidates: Vec<(String, usize)> = Vec::new();
        for branch in &tracked {
            if branch.name == base_branch {
                continue;
            }
            if !merged_state_by_branch
                .get(&branch.name)
                .copied()
                .unwrap_or_else(|| {
                    branch
                        .cached_pr_state
                        .as_deref()
                        .is_some_and(|state| state.eq_ignore_ascii_case("merged"))
                })
            {
                continue;
            }

            let mut depth = 0usize;
            let mut cursor = branch.parent_branch_id;
            while let Some(parent_id) = cursor {
                depth += 1;
                cursor = by_id.get(&parent_id).and_then(|p| p.parent_branch_id);
            }
            prune_candidates.push((branch.name.clone(), depth));
        }
        prune_candidates.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        for (branch, _) in prune_candidates {
            ops.push(SyncOp::PruneMergedBranch { branch });
        }
    }

    let base_url = git
        .remote_web_url(&sync_remote)?
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
                    pr_number: pr_by_branch.get(&parent.name).map(|p| p.number),
                    pr_url: pr_by_branch.get(&parent.name).and_then(|p| p.url.clone()),
                });
            let first_child = children.get(&branch.id).and_then(|ids| {
                ids.iter()
                    .filter_map(|id| by_id.get(id))
                    .map(|child| ManagedBranchRef {
                        branch: child.name.clone(),
                        pr_number: pr_by_branch.get(&child.name).map(|p| p.number),
                        pr_url: pr_by_branch.get(&child.name).and_then(|p| p.url.clone()),
                    })
                    .min_by(|a, b| a.branch.cmp(&b.branch))
            });
            let pr_root = pr
                .url
                .as_deref()
                .and_then(repo_root_from_pr_url)
                .unwrap_or(base_url.as_str());
            let base_commit_url = git
                .merge_base(&branch.name, base_branch)
                .ok()
                .map(|sha| format!("{}/commit/{sha}", pr_root.trim_end_matches('/')));
            let managed_section = managed_pr_section(
                pr_root,
                base_branch,
                base_commit_url.as_deref(),
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

    if needs_fetch {
        ops.insert(
            0,
            SyncOp::Fetch {
                remote: sync_remote.clone(),
            },
        );
    }

    let plan = SyncPlan {
        base_branch: base_branch.to_string(),
        ops,
    };
    let timing = SyncPlanTiming {
        setup: setup_elapsed,
        pr_lookup: pr_lookup_elapsed,
        assemble: assemble_started.elapsed(),
    };
    Ok((plan, timing))
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
                SyncOp::UpdateBaseToMergeCommit {
                    branch,
                    merge_commit,
                } => {
                    git.fast_forward_branch(branch, merge_commit)?;
                    let sha = git.head_sha(branch)?;
                    db.set_sync_sha(branch, &sha)?;
                }
                SyncOp::Restack {
                    branch,
                    onto,
                    old_base,
                    ..
                } => {
                    let old_base = if let Some(old_base) = old_base {
                        old_base.clone()
                    } else {
                        git.merge_base(branch, onto)?
                    };
                    if git.commit_distance(&old_base, branch)? == 0 {
                        git.rebase_onto(branch, &old_base, onto)?;
                        let sha = git.head_sha(branch)?;
                        db.set_sync_sha(branch, &sha)?;
                        continue;
                    }
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
                SyncOp::PruneMergedBranch { branch } => {
                    if git.branch_exists(branch)? {
                        if git.current_branch()? == *branch {
                            git.checkout_branch(&plan.base_branch)?;
                        }
                        git.delete_local_branch(branch)?;
                    }
                    if db.branch_by_name(branch)?.is_some() {
                        db.splice_out_branch(branch)?;
                    }
                }
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
