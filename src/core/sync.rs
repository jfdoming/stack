use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};

use crate::db::{BranchRecord, Database};
use crate::git::{Git, StashHandle};
use crate::provider::{PrIdentity, PrState, Provider};
use crate::util::pr_body::{ManagedBranchRef, managed_pr_section, merge_managed_pr_section};
use crate::util::url::github_repo_slug_from_web_url;
use crate::views::{OperationView, SyncPlanView};

#[derive(Debug, Clone)]
pub enum SyncOp {
    Fetch {
        remote: String,
        expected_base_ref: Option<String>,
        expected_base_sha: Option<String>,
    },
    UpdateBaseToMergeCommit {
        branch: String,
        merge_commit: String,
    },
    Restack {
        branch: String,
        onto: String,
        old_base: String,
        expected_head: String,
        reason: String,
    },
    UpdateSha {
        branch: String,
        sha: String,
    },
    UpdatePrBody {
        branch: String,
        identity: PrIdentity,
        body: String,
    },
    UpdatePrBase {
        branch: String,
        identity: PrIdentity,
        base: String,
    },
    PruneMergedBranch {
        branch: String,
        merged_head_oid: Option<String>,
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
                SyncOp::Fetch { remote, .. } => operations.push(OperationView {
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
                    expected_head: _,
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
                    branch, identity, ..
                } => operations.push(OperationView {
                    kind: "update_pr_body".to_string(),
                    branch: branch.clone(),
                    onto: None,
                    details: format!("pr #{}", identity.number),
                }),
                SyncOp::UpdatePrBase {
                    branch,
                    identity,
                    base,
                } => operations.push(OperationView {
                    kind: "update_pr_base".to_string(),
                    branch: branch.clone(),
                    onto: Some(base.clone()),
                    details: format!("pr #{} -> {base}", identity.number),
                }),
                SyncOp::PruneMergedBranch { branch, .. } => operations.push(OperationView {
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
        old_base: String,
    }

    let setup_started = Instant::now();
    let sync_remote = git.preferred_sync_remote(base_remote)?;
    let tracked = db.list_branches()?;
    let mut branch_exists: HashMap<String, bool> = HashMap::new();
    for branch in &tracked {
        branch_exists.insert(branch.name.clone(), git.branch_exists(&branch.name)?);
    }
    let remote_base_ref = format!("{sync_remote}/{base_branch}");
    let full_remote_base_ref = format!("refs/remotes/{sync_remote}/{base_branch}");
    let should_inspect_remote_base = git.has_remote(&sync_remote)?;
    let advertised_remote_base = should_inspect_remote_base
        .then(|| git.advertised_remote_branch_sha(&sync_remote, base_branch))
        .transpose()?
        .flatten();
    let tracked_remote_base = git
        .ref_exists(&remote_base_ref)?
        .then(|| git.resolve_commit(&remote_base_ref))
        .transpose()?;
    let remote_base_needs_fetch = advertised_remote_base
        .as_ref()
        .is_some_and(|advertised| tracked_remote_base.as_deref() != Some(advertised.as_str()));
    let remote_base_target = advertised_remote_base
        .clone()
        .or_else(|| tracked_remote_base.clone())
        .unwrap_or_else(|| base_branch.to_string());
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
    let mut needs_fetch = remote_base_needs_fetch;
    let mut sha_updates = Vec::new();
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
            db.set_pr_cache(&branch.name, Some(pr.identity.number), Some(state))?;
            is_merged_pr = matches!(pr.state, PrState::Merged);

            if matches!(pr.state, PrState::Merged) {
                let merge_commit_oid = pr.merge_commit_oid.clone();
                merged_restack_base = Some(
                    merge_commit_oid
                        .clone()
                        .unwrap_or_else(|| remote_base_target.clone()),
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
            let new_base = merged_restack_base.unwrap_or_else(|| remote_base_target.clone());
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
                                old_base: safe_restack_old_base(git, &branch.name, &child.name)?,
                            });
                        }
                    }
                }
            }
        }

        if !is_merged_pr
            && let Some(parent_id) = branch.parent_branch_id
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
                    remote_base_target.clone()
                } else {
                    parent.name.clone()
                };
                let parent_onto_is_local = git.ref_exists(&parent_onto)?;
                if !parent_onto_is_local || !git.is_ancestor(&parent_onto, &branch.name)? {
                    queue.push_back(RestackCandidate {
                        branch: branch.name.clone(),
                        onto: parent_onto,
                        old_base: safe_restack_old_base(git, &parent.name, &branch.name)?,
                    });
                }
            }
        }
        if !is_merged_pr
            && branch.name != base_branch
            && branch.last_synced_head_sha.as_deref() != Some(current_sha.as_str())
        {
            sha_updates.push(SyncOp::UpdateSha {
                branch: branch.name.clone(),
                sha: current_sha,
            });
        }
    }

    let base_current_sha = current_sha_by_branch.get(base_branch).cloned();
    let mut base_will_move = false;
    if let Some(merge_commit) = base_merge_commit_to_apply {
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
            base_will_move = true;
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
    if !base_will_move
        && let Some(base_sha) = base_current_sha
        && let Some(base) = tracked.iter().find(|branch| branch.name == base_branch)
        && base.last_synced_head_sha.as_deref() != Some(base_sha.as_str())
    {
        sha_updates.push(SyncOp::UpdateSha {
            branch: base_branch.to_string(),
            sha: base_sha,
        });
    }

    let mut restack_by_branch: HashMap<String, RestackCandidate> = HashMap::new();
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
        if restack_by_branch.contains_key(&item.branch) {
            continue;
        }
        restack_by_branch.insert(item.branch.clone(), item.clone());
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
                        old_base: safe_restack_old_base(git, &item.branch, &child.name)?,
                    });
                }
            }
        }
    }
    let mut restack_candidates: Vec<RestackCandidate> = restack_by_branch.into_values().collect();
    restack_candidates.sort_by(|a, b| {
        branch_depth(&a.branch, &tracked, &by_id)
            .cmp(&branch_depth(&b.branch, &tracked, &by_id))
            .then(a.branch.cmp(&b.branch))
    });
    let restacked_branches: HashSet<String> = restack_candidates
        .iter()
        .map(|candidate| candidate.branch.clone())
        .collect();
    for item in restack_candidates {
        needs_fetch = true;
        ops.push(SyncOp::Restack {
            expected_head: current_sha_by_branch
                .get(&item.branch)
                .cloned()
                .ok_or_else(|| anyhow!("missing planned head for '{}'", item.branch))?,
            branch: item.branch,
            onto: item.onto,
            old_base: item.old_base,
            reason: "parent updated or merged".to_string(),
        });
    }
    ops.extend(sha_updates.into_iter().filter(|update| {
        !matches!(
            update,
            SyncOp::UpdateSha { branch, .. } if restacked_branches.contains(branch)
        )
    }));

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
    let mut all_local_heads_are_prune_safe = true;
    for branch in tracked.iter().filter(|branch| branch.name != base_branch) {
        if !branch_exists.get(&branch.name).copied().unwrap_or(false) {
            continue;
        }
        let Some(current_sha) = current_sha_by_branch.get(&branch.name) else {
            all_local_heads_are_prune_safe = false;
            break;
        };
        let Some(merged_head) = pr_by_branch.get(&branch.name).and_then(|pr| {
            matches!(pr.state, PrState::Merged)
                .then_some(pr.head_ref_oid.as_deref())
                .flatten()
        }) else {
            all_local_heads_are_prune_safe = false;
            break;
        };
        let is_safe = current_sha == merged_head
            || (git.ref_exists(merged_head)? && git.is_ancestor(current_sha, merged_head)?);
        if !is_safe {
            all_local_heads_are_prune_safe = false;
            break;
        }
    }
    if stack_fully_merged && all_local_heads_are_prune_safe {
        let mut prune_candidates: Vec<(String, usize, Option<String>)> = Vec::new();
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
            let merged_head_oid = pr_by_branch
                .get(&branch.name)
                .and_then(|pr| pr.head_ref_oid.clone());
            prune_candidates.push((branch.name.clone(), depth, merged_head_oid));
        }
        prune_candidates.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        for (branch, _, merged_head_oid) in prune_candidates {
            ops.push(SyncOp::PruneMergedBranch {
                branch,
                merged_head_oid,
            });
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
                    pr_number: pr_by_branch.get(&parent.name).map(|p| p.identity.number),
                    pr_url: pr_by_branch.get(&parent.name).and_then(|p| p.url.clone()),
                });
            let first_child = children.get(&branch.id).and_then(|ids| {
                ids.iter()
                    .filter_map(|id| by_id.get(id))
                    .map(|child| ManagedBranchRef {
                        branch: child.name.clone(),
                        pr_number: pr_by_branch.get(&child.name).map(|p| p.identity.number),
                        pr_url: pr_by_branch.get(&child.name).and_then(|p| p.url.clone()),
                    })
                    .min_by(|a, b| a.branch.cmp(&b.branch))
            });
            let expected_pr_base = branch
                .parent_branch_id
                .and_then(|parent_id| by_id.get(&parent_id))
                .map(|parent| parent.name.clone())
                .unwrap_or_else(|| base_branch.to_string());
            if pr.base_ref_name.as_deref() != Some(expected_pr_base.as_str()) {
                if can_update_pr_base(
                    db,
                    git,
                    base_branch,
                    &branch.name,
                    &expected_pr_base,
                    pr.url.as_deref(),
                ) {
                    ops.push(SyncOp::UpdatePrBase {
                        branch: branch.name.clone(),
                        identity: pr.identity.clone(),
                        base: expected_pr_base,
                    });
                } else {
                    eprintln!(
                        "warning: skipping PR base update for '{}' because '{}' is not in the PR repository",
                        branch.name, expected_pr_base
                    );
                }
            }
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
                    identity: pr.identity.clone(),
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
                expected_base_ref: advertised_remote_base
                    .as_ref()
                    .map(|_| full_remote_base_ref),
                expected_base_sha: advertised_remote_base,
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

fn safe_restack_old_base(git: &Git, parent: &str, child: &str) -> Result<String> {
    let parent_sha = git.head_sha(parent)?;
    if git.is_ancestor(&parent_sha, child)? {
        return Ok(parent_sha);
    }

    let fork_point = git.merge_base_fork_point(parent, child)?.ok_or_else(|| {
        anyhow!(
            "cannot safely restack '{}' onto rewritten parent '{}': the prior fork point is unavailable",
            child,
            parent
        )
    })?;
    if !git.ref_exists(&fork_point)? || !git.is_ancestor(&fork_point, child)? {
        return Err(anyhow!(
            "cannot safely restack '{}' onto rewritten parent '{}': the prior fork point is invalid",
            child,
            parent
        ));
    }
    Ok(fork_point)
}

fn branch_depth(
    branch_name: &str,
    tracked: &[BranchRecord],
    by_id: &HashMap<i64, BranchRecord>,
) -> usize {
    let mut depth = 0;
    let mut cursor = tracked
        .iter()
        .find(|branch| branch.name == branch_name)
        .and_then(|branch| branch.parent_branch_id);
    while let Some(parent_id) = cursor {
        depth += 1;
        cursor = by_id
            .get(&parent_id)
            .and_then(|parent| parent.parent_branch_id);
    }
    depth
}

pub fn execute_sync_plan(
    db: &Database,
    git: &Git,
    provider: &dyn Provider,
    plan: &SyncPlan,
) -> Result<()> {
    let starting_branch = git.current_branch()?;
    let worktree_dirty = git.is_worktree_dirty()?;
    let starting_branch_will_be_pruned = plan.ops.iter().any(|op| {
        matches!(
            op,
            SyncOp::PruneMergedBranch { branch, .. } if branch == &starting_branch
        )
    });
    if worktree_dirty && starting_branch_will_be_pruned {
        return Err(anyhow!(
            "cannot prune the checked-out branch with uncommitted changes; commit or stash the changes, or switch to '{}' before syncing",
            plan.base_branch
        ));
    }
    let mut stash: Option<StashHandle> = None;
    if worktree_dirty {
        eprintln!("warning: worktree is dirty; auto-stashing local changes");
        stash = git.stash_push("stack-sync-auto-stash")?;
    }

    let run_id = db.record_sync_start()?;
    let mut status = "success";
    let mut summary = None;
    let replay_supported = git.supports_replay();
    let mut pending_sync_shas: HashMap<String, String> = HashMap::new();

    let op_result: Result<()> = (|| {
        for op in &plan.ops {
            match op {
                SyncOp::Fetch {
                    remote,
                    expected_base_ref,
                    expected_base_sha,
                } => {
                    git.fetch_remote(remote)?;
                    if let (Some(reference), Some(expected)) =
                        (expected_base_ref.as_deref(), expected_base_sha.as_deref())
                    {
                        let actual = git.resolve_commit(reference)?;
                        if actual != expected {
                            return Err(anyhow!(
                                "remote base changed while sync was being applied; rerun sync to build a fresh plan"
                            ));
                        }
                    }
                }
                SyncOp::UpdateBaseToMergeCommit {
                    branch,
                    merge_commit,
                } => {
                    git.fast_forward_branch(branch, merge_commit)?;
                    let sha = git.head_sha(branch)?;
                    pending_sync_shas.insert(branch.clone(), sha);
                }
                SyncOp::Restack {
                    branch,
                    onto,
                    old_base,
                    expected_head,
                    ..
                } => {
                    let actual_head = git.head_sha(branch)?;
                    if actual_head != *expected_head {
                        return Err(anyhow!(
                            "branch '{branch}' changed after the sync plan was built; rerun sync to review a fresh plan"
                        ));
                    }
                    if git.commit_distance(old_base, expected_head)? == 0 {
                        git.rebase_onto(branch, expected_head, old_base, onto)?;
                        let sha = git.head_sha(branch)?;
                        pending_sync_shas.insert(branch.clone(), sha);
                        continue;
                    }
                    if replay_supported {
                        if let Err(err) = git.replay_onto(branch, expected_head, old_base, onto) {
                            if git.head_sha(branch)? != *expected_head {
                                return Err(anyhow!(
                                    "branch '{branch}' changed after the sync plan was built; rerun sync to review a fresh plan"
                                ));
                            }
                            let reason = summarize_replay_error(&err);
                            eprintln!(
                                "warning: git replay is unavailable for '{branch}' ({reason}); falling back to rebase"
                            );
                            git.rebase_onto(branch, expected_head, old_base, onto)?;
                        }
                    } else {
                        eprintln!("warning: git replay unavailable; using rebase for {branch}");
                        git.rebase_onto(branch, expected_head, old_base, onto)?;
                    }
                    let sha = git.head_sha(branch)?;
                    pending_sync_shas.insert(branch.clone(), sha);
                }
                SyncOp::UpdateSha { branch, sha } => {
                    pending_sync_shas.insert(branch.clone(), sha.clone());
                }
                SyncOp::UpdatePrBody { identity, body, .. } => {
                    provider.update_pr_body(identity, body)?
                }
                SyncOp::UpdatePrBase { identity, base, .. } => {
                    provider.update_pr_base(identity, base)?
                }
                SyncOp::PruneMergedBranch {
                    branch,
                    merged_head_oid,
                } => {
                    if git.branch_exists(branch)? {
                        let expected_head = merged_head_oid.as_deref().ok_or_else(|| {
                            anyhow!(
                                "refusing to prune '{}': merged PR head is unavailable",
                                branch
                            )
                        })?;
                        let current_head = git.head_sha(branch)?;
                        let still_safe = current_head == expected_head
                            || (git.ref_exists(expected_head)?
                                && git.is_ancestor(&current_head, expected_head)?);
                        if !still_safe {
                            return Err(anyhow!(
                                "refusing to prune '{}': branch changed after the sync plan was built",
                                branch
                            ));
                        }
                        if git.current_branch()? == *branch {
                            git.checkout_branch(&plan.base_branch)?;
                        }
                        git.delete_local_branch_if_unchanged(branch, &current_head)?;
                    }
                    if db.branch_by_name(branch)?.is_some() {
                        db.splice_out_branch(branch)?;
                    }
                }
            }
        }
        for (branch, sha) in &pending_sync_shas {
            if db.branch_by_name(branch)?.is_some() {
                db.set_sync_sha(branch, sha)?;
            }
        }
        Ok(())
    })();

    let restore_branch_result = restore_starting_branch(git, &starting_branch);

    let mut stash_restore_error = None;
    if let Some(stash_handle) = stash {
        let original_branch_restored = restore_branch_result.is_ok()
            && !starting_branch.is_empty()
            && git
                .current_branch()
                .is_ok_and(|current| current == starting_branch);
        if original_branch_restored {
            if let Err(err) = git.stash_restore(&stash_handle) {
                stash_restore_error = Some(anyhow!(
                    "could not auto-restore stash {} on '{}': {err}",
                    stash_handle.reference,
                    starting_branch
                ));
            } else {
                eprintln!(
                    "note: restored auto-stash {}; its recovery entry remains in git stash list",
                    stash_handle.reference
                );
            }
        } else {
            stash_restore_error = Some(anyhow!(
                "auto-stash {} was left intact because the prior branch '{}' could not be restored safely",
                stash_handle.reference,
                starting_branch
            ));
        }
    }

    let mut result = match (op_result, restore_branch_result) {
        (Err(op_err), Err(restore_err)) => Err(anyhow!(format_sync_failure(
            &op_err,
            Some(&restore_err),
            &starting_branch
        ))),
        (Err(op_err), Ok(())) => Err(anyhow!(format_sync_failure(
            &op_err,
            None,
            &starting_branch
        ))),
        (Ok(()), Err(restore_err)) => Err(anyhow!(
            "failed to restore prior branch '{}': {restore_err}",
            starting_branch
        )),
        (Ok(()), Ok(())) => Ok(()),
    };
    if let Some(stash_err) = stash_restore_error {
        result = match result {
            Ok(()) => Err(stash_err),
            Err(err) => Err(anyhow!("{err}; {stash_err}")),
        };
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
    if !git.branch_exists(starting_branch)? {
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

fn format_sync_failure(
    op_err: &anyhow::Error,
    restore_err: Option<&anyhow::Error>,
    starting_branch: &str,
) -> String {
    let op_msg = op_err.to_string();
    if is_merge_conflict_error(&op_msg) {
        return format_merge_conflict_guidance(&op_msg);
    }

    if let Some(restore_err) = restore_err {
        return format!(
            "{op_msg}; additionally failed to restore prior branch '{}': {}",
            starting_branch, restore_err
        );
    }

    op_msg
}

fn is_merge_conflict_error(msg: &str) -> bool {
    msg.contains("Resolve all conflicts manually")
        || msg.contains("git rebase --continue")
        || msg.contains("could not apply")
}

fn format_merge_conflict_guidance(op_msg: &str) -> String {
    let branch = conflicted_branch_from_sync_error(op_msg).unwrap_or("the branch");
    format!(
        "sync stopped due to merge conflicts while restacking '{branch}'. Resolve the conflicts, run `git rebase --continue`, then rerun `stack sync` if additional restacks remain. Use `git rebase --abort` to cancel the in-progress restack."
    )
}

fn conflicted_branch_from_sync_error(msg: &str) -> Option<&str> {
    if let Some(tail) = msg.strip_prefix("restack of '")
        && let Some((branch, _)) = tail.split_once('\'')
    {
        return Some(branch);
    }
    let marker = "\", \"";
    let rebase_idx = msg.find("[\"rebase\", \"--onto\", \"")?;
    let tail = &msg[rebase_idx..];
    let first = tail.find(marker)?;
    let after_first = &tail[first + marker.len()..];
    let second = after_first.find(marker)?;
    let after_second = &after_first[second + marker.len()..];
    let third = after_second.find(marker)?;
    Some(&after_second[..third])
}

fn can_update_pr_base(
    db: &Database,
    git: &Git,
    stack_base: &str,
    branch: &str,
    expected_base: &str,
    pr_url: Option<&str>,
) -> bool {
    let Some(pr_url) = pr_url else {
        return true;
    };
    let Some(pr_repo_slug) = github_repo_slug_from_web_url(pr_url) else {
        return true;
    };
    let base_repo_slug = if expected_base == stack_base {
        db.repo_meta()
            .ok()
            .and_then(|meta| meta.canonical_repo)
            .or_else(|| {
                git.remote_web_url("upstream")
                    .ok()
                    .flatten()
                    .and_then(|url| github_repo_slug_from_web_url(&url))
            })
            .or_else(|| {
                git.remote_web_url("origin")
                    .ok()
                    .flatten()
                    .and_then(|url| github_repo_slug_from_web_url(&url))
            })
    } else {
        git.configured_remote_for_branch(expected_base)
            .ok()
            .flatten()
            .or_else(|| git.configured_remote_for_branch(branch).ok().flatten())
            .and_then(|remote| {
                git.remote_push_web_url(&remote)
                    .ok()
                    .flatten()
                    .or_else(|| git.remote_web_url(&remote).ok().flatten())
            })
            .and_then(|url| github_repo_slug_from_web_url(&url))
    };
    let Some(base_repo_slug) = base_repo_slug else {
        return true;
    };
    pr_repo_slug == base_repo_slug
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;

    use super::{format_sync_failure, summarize_replay_error};

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

    #[test]
    fn format_sync_failure_simplifies_rebase_conflict_guidance() {
        let op_err = anyhow!(
            "git command failed [\"rebase\", \"--onto\", \"parent\", \"base\", \"branch\"]: error: could not apply abc123\nhint: Resolve all conflicts manually, mark them as resolved with\nhint: \"git add/rm <conflicted_files>\", then run \"git rebase --continue\".\nhint: You can instead skip this commit: run \"git rebase --skip\".\nhint: To abort and get back to the state before \"git rebase\", run \"git rebase --abort\"."
        );
        let restore_err = anyhow!(
            "git command failed [\"checkout\", \"main\"]: error: you need to resolve your current index first"
        );

        let got = format_sync_failure(&op_err, Some(&restore_err), "main");

        assert!(got.contains("sync stopped due to merge conflicts"));
        assert!(got.contains("git rebase --continue"));
        assert!(got.contains("git rebase --abort"));
        assert!(!got.contains("failed to restore prior branch"));
    }

    #[test]
    fn format_sync_failure_keeps_restore_error_for_non_conflict_failures() {
        let op_err = anyhow!("git command failed [\"fetch\"]: fatal: bad remote");
        let restore_err =
            anyhow!("git command failed [\"checkout\", \"main\"]: some restore failure");

        let got = format_sync_failure(&op_err, Some(&restore_err), "main");

        assert!(got.contains("failed to restore prior branch 'main'"));
        assert!(got.contains("fatal: bad remote"));
    }
}
