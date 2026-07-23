use std::collections::{HashMap, HashSet};
use std::io::{IsTerminal, stdin, stdout};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, anyhow};
use dialoguer::{Select, theme::ColorfulTheme};
use serde::Serialize;

use crate::db::{BranchRecord, Database, RepoMeta};
use crate::git::{Git, RemoteInfo};
use crate::provider::Provider;
use crate::ui::interaction::prompt_or_cancel;
use crate::util::url::github_repo_slug_from_web_url;

const PLACEMENT_CACHE_TTL_SECONDS: i64 = 24 * 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PushTarget {
    Auto,
    Upstream,
    Fork,
}

impl PushTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Upstream => "upstream",
            Self::Fork => "fork",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "upstream" => Some(Self::Upstream),
            "fork" => Some(Self::Fork),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ResolvedPlacement {
    pub branch: String,
    pub remote: String,
    pub repository: Option<String>,
    pub push_target: PushTarget,
    pub decision_source: String,
    pub canonical_repository: Option<String>,
    pub canonical_remote: Option<String>,
    pub fork_repository: Option<String>,
    pub push_permission: Option<String>,
    pub permission_checked_at: Option<i64>,
    pub cache_age_seconds: Option<i64>,
    pub cache_state: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlacementStatus {
    pub push_target: Option<String>,
    pub resolved_target: Option<PushTarget>,
    pub canonical_repository: Option<String>,
    pub canonical_remote: Option<String>,
    pub fork_repository: Option<String>,
    pub fork_remote: Option<String>,
    pub push_permission: Option<String>,
    pub permission_checked_at: Option<i64>,
    pub cache_age_seconds: Option<i64>,
    pub cache_state: String,
}

pub struct PlacementRequest<'a> {
    pub records: &'a [BranchRecord],
    pub branches: &'a [String],
    pub base_branch: &'a str,
    pub requested: Option<PushTarget>,
    pub yes: bool,
}

#[derive(Debug, Clone)]
struct Topology {
    canonical_repo: Option<String>,
    fork_repo: Option<String>,
    canonical_remote: Option<String>,
    fork_remote: Option<String>,
    permission: Option<String>,
    checked_at: Option<i64>,
    detected: bool,
}

pub fn placement_status(db: &Database, git: &Git) -> Result<PlacementStatus> {
    let meta = db.repo_meta()?;
    let topology = topology_from_local(git, &meta)?;
    let resolved_target = meta
        .push_target
        .as_deref()
        .and_then(PushTarget::parse)
        .map(|target| {
            effective_target(
                target,
                topology.permission.as_deref(),
                topology.fork_repo.is_some(),
            )
        });
    let cache_age_seconds = cache_age_seconds(topology.checked_at);
    Ok(PlacementStatus {
        push_target: meta.push_target,
        resolved_target,
        canonical_repository: topology.canonical_repo,
        canonical_remote: topology.canonical_remote,
        fork_repository: topology.fork_repo,
        fork_remote: topology.fork_remote,
        push_permission: topology.permission,
        permission_checked_at: topology.checked_at,
        cache_age_seconds,
        cache_state: cache_state(cache_age_seconds).to_string(),
    })
}

pub fn resolve_push_placements(
    db: &Database,
    git: &Git,
    provider: &dyn Provider,
    request: PlacementRequest<'_>,
) -> Result<Vec<ResolvedPlacement>> {
    let PlacementRequest {
        records,
        branches,
        base_branch,
        requested,
        yes,
    } = request;
    if branches.is_empty() {
        return Ok(Vec::new());
    }
    let by_id: HashMap<i64, &BranchRecord> =
        records.iter().map(|record| (record.id, record)).collect();
    let by_name: HashMap<&str, &BranchRecord> = records
        .iter()
        .map(|record| (record.name.as_str(), record))
        .collect();
    let needs_policy = branches.iter().any(|branch| {
        inherited_configured_remote(
            git,
            &by_id,
            by_name.get(branch.as_str()).copied(),
            base_branch,
        )
        .ok()
        .flatten()
        .is_none()
    });

    let mut meta = db.repo_meta()?;
    let should_detect = needs_policy;
    let mut topology = resolve_topology(db, git, provider, &meta, should_detect)?;
    meta = db.repo_meta()?;

    let (policy, policy_source) = if let Some(target) = requested {
        (target, "command flag")
    } else if let Some(target) = meta.push_target.as_deref().and_then(PushTarget::parse) {
        (target, "repository config")
    } else if needs_policy && stdout().is_terminal() && stdin().is_terminal() && !yes {
        let target = prompt_for_target(&topology)?;
        db.set_push_target(target.as_str())?;
        (target, "first-push prompt")
    } else if needs_policy && topology.detected {
        db.set_push_target("auto")?;
        // set_push_target(auto) intentionally clears permission; restore this successful detection.
        if let (Some(canonical), Some(checked_at)) =
            (topology.canonical_repo.as_deref(), topology.checked_at)
        {
            db.set_placement_cache(
                canonical,
                topology.fork_repo.as_deref(),
                topology.permission.as_deref(),
                checked_at,
            )?;
        }
        (PushTarget::Auto, "automatic detection")
    } else {
        (PushTarget::Auto, "safe fallback")
    };

    // Re-read local/cache state after the first-push choice was persisted.
    topology = topology_from_local(git, &db.repo_meta()?)?.with_detected(topology.detected);
    let selected = effective_target(
        policy,
        topology.permission.as_deref(),
        topology.fork_repo.is_some(),
    );
    let selected_remote = remote_for_target(&topology, selected)?;
    let selected_repo = repo_for_remote(git, &selected_remote)?;

    let mut branch_resolver = BranchRemoteResolver {
        git,
        by_id: &by_id,
        by_name: &by_name,
        base_branch,
        policy_remote: &selected_remote,
        memo: HashMap::new(),
        resolving: HashSet::new(),
    };
    for branch in branches {
        branch_resolver.resolve(branch)?;
    }
    let memo = branch_resolver.memo;

    if requested.is_some() {
        for branch in branches {
            if let Some((existing, _)) = memo.get(branch)
                && repo_for_remote(git, existing)? != selected_repo
            {
                return Err(anyhow!(
                    "push target '{}' conflicts with existing upstream '{}' for branch '{}'; existing upstreams are never migrated automatically",
                    selected.as_str(),
                    existing,
                    branch
                ));
            }
        }
    }

    for branch in branches {
        let Some(record) = by_name.get(branch.as_str()) else {
            continue;
        };
        if let Some(parent) = record.parent_branch_id.and_then(|id| by_id.get(&id))
            && parent.name != base_branch
            && let Some((branch_remote, _)) = memo.get(branch)
            && let Some((parent_remote, _)) = memo.get(&parent.name)
            && repo_for_remote(git, branch_remote)? != repo_for_remote(git, parent_remote)?
        {
            return Err(anyhow!(
                "stack placement conflict: '{}' and its parent '{}' resolve to different repositories",
                branch,
                parent.name
            ));
        }
    }

    branches
        .iter()
        .map(|branch| {
            let (remote, source) = memo
                .get(branch)
                .cloned()
                .unwrap_or_else(|| (selected_remote.clone(), policy_source.to_string()));
            let repository = repo_for_remote(git, &remote)?;
            let target = classify_repo(&topology, repository.as_deref()).unwrap_or(selected);
            let cache_age_seconds = cache_age_seconds(topology.checked_at);
            Ok(ResolvedPlacement {
                branch: branch.clone(),
                remote,
                repository,
                push_target: target,
                decision_source: if source == "policy" {
                    policy_source.to_string()
                } else {
                    source
                },
                canonical_repository: topology.canonical_repo.clone(),
                canonical_remote: topology.canonical_remote.clone(),
                fork_repository: topology.fork_repo.clone(),
                push_permission: topology.permission.clone(),
                permission_checked_at: topology.checked_at,
                cache_age_seconds,
                cache_state: cache_state(cache_age_seconds).to_string(),
            })
        })
        .collect()
}

struct BranchRemoteResolver<'a> {
    git: &'a Git,
    by_id: &'a HashMap<i64, &'a BranchRecord>,
    by_name: &'a HashMap<&'a str, &'a BranchRecord>,
    base_branch: &'a str,
    policy_remote: &'a str,
    memo: HashMap<String, (String, String)>,
    resolving: HashSet<String>,
}

impl BranchRemoteResolver<'_> {
    fn resolve(&mut self, branch: &str) -> Result<(String, String)> {
        if let Some(value) = self.memo.get(branch) {
            return Ok(value.clone());
        }
        if !self.resolving.insert(branch.to_string()) {
            return Err(anyhow!(
                "cycle while resolving push placement for '{branch}'"
            ));
        }
        let resolved = if let Some(remote) = self.git.configured_remote_for_branch(branch)? {
            (remote, "existing upstream".to_string())
        } else if let Some(record) = self.by_name.get(branch).copied() {
            if let Some(parent) = record.parent_branch_id.and_then(|id| self.by_id.get(&id)) {
                if parent.name == self.base_branch {
                    (self.policy_remote.to_string(), "policy".to_string())
                } else {
                    let (remote, _) = self.resolve(&parent.name)?;
                    (remote, "stack ancestor".to_string())
                }
            } else {
                (self.policy_remote.to_string(), "policy".to_string())
            }
        } else {
            (self.policy_remote.to_string(), "policy".to_string())
        };
        self.resolving.remove(branch);
        self.memo.insert(branch.to_string(), resolved.clone());
        Ok(resolved)
    }
}

fn inherited_configured_remote<'a>(
    git: &Git,
    by_id: &HashMap<i64, &'a BranchRecord>,
    mut record: Option<&'a BranchRecord>,
    base_branch: &str,
) -> Result<Option<String>> {
    let mut seen = HashSet::new();
    while let Some(current) = record {
        if !seen.insert(current.id) {
            break;
        }
        if let Some(remote) = git.configured_remote_for_branch(&current.name)? {
            return Ok(Some(remote));
        }
        record = current
            .parent_branch_id
            .and_then(|id| by_id.get(&id).copied())
            .filter(|parent| parent.name != base_branch);
    }
    Ok(None)
}

fn resolve_topology(
    db: &Database,
    git: &Git,
    provider: &dyn Provider,
    meta: &RepoMeta,
    detect: bool,
) -> Result<Topology> {
    let mut topology = topology_from_local(git, meta)?;
    let now = unix_timestamp();
    let cache_fresh = topology
        .checked_at
        .is_some_and(|checked| now.saturating_sub(checked) <= PLACEMENT_CACHE_TTL_SECONDS)
        && topology.canonical_repo.is_some();
    if !detect || cache_fresh {
        return Ok(topology);
    }

    let query_repo = topology
        .canonical_repo
        .clone()
        .or_else(|| topology.fork_repo.clone());
    let Some(query_repo) = query_repo else {
        return Ok(topology);
    };
    let Some(info) = provider.repository_info(&query_repo)? else {
        return Ok(topology);
    };
    let (canonical, fork, permission) = if let Some(parent) = info.parent_name_with_owner {
        (
            parent,
            Some(info.name_with_owner),
            info.parent_viewer_permission,
        )
    } else {
        (
            info.name_with_owner,
            topology
                .fork_repo
                .filter(|fork| Some(fork) != topology.canonical_repo.as_ref()),
            info.viewer_permission,
        )
    };
    db.set_placement_cache(&canonical, fork.as_deref(), permission.as_deref(), now)?;
    topology = topology_from_local(git, &db.repo_meta()?)?;
    topology.detected = true;
    Ok(topology)
}

fn topology_from_local(git: &Git, meta: &RepoMeta) -> Result<Topology> {
    let remotes = git.remote_infos()?;
    let upstream = remotes.iter().find(|remote| remote.name == "upstream");
    let origin = remotes.iter().find(|remote| remote.name == "origin");
    let canonical_repo = meta
        .canonical_repo
        .clone()
        .or_else(|| upstream.and_then(fetch_repo))
        .or_else(|| origin.and_then(fetch_repo))
        .or_else(|| remotes.iter().find_map(fetch_repo))
        .or_else(|| remotes.iter().find_map(push_repo));
    let fork_repo = meta
        .fork_repo
        .clone()
        .or_else(|| {
            origin
                .and_then(push_repo)
                .filter(|repo| Some(repo) != canonical_repo.as_ref())
        })
        .or_else(|| {
            remotes
                .iter()
                .filter_map(push_repo)
                .find(|repo| Some(repo) != canonical_repo.as_ref())
        });
    let canonical_remote = canonical_repo
        .as_deref()
        .and_then(|repo| find_remote(&remotes, repo, Some("upstream")));
    let fork_remote = fork_repo
        .as_deref()
        .and_then(|repo| find_remote(&remotes, repo, Some("origin")));
    Ok(Topology {
        canonical_repo,
        fork_repo,
        canonical_remote,
        fork_remote,
        permission: meta.push_permission.clone(),
        checked_at: meta.permission_checked_at,
        detected: false,
    })
}

impl Topology {
    fn with_detected(mut self, detected: bool) -> Self {
        self.detected = detected;
        self
    }
}

fn prompt_for_target(topology: &Topology) -> Result<PushTarget> {
    let recommended = effective_target(
        PushTarget::Auto,
        topology.permission.as_deref(),
        topology.fork_repo.is_some(),
    );
    let options = [
        format!("Auto (currently {})", recommended.as_str()),
        "Always upstream".to_string(),
        "Always fork".to_string(),
    ];
    let selected = prompt_or_cancel(
        Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Choose where new stacks should be pushed")
            .items(&options)
            .default(0)
            .interact(),
    )?;
    Ok(match selected {
        1 => PushTarget::Upstream,
        2 => PushTarget::Fork,
        _ => PushTarget::Auto,
    })
}

fn effective_target(target: PushTarget, permission: Option<&str>, has_fork: bool) -> PushTarget {
    if target != PushTarget::Auto {
        return target;
    }
    if permission.is_some_and(is_writable_permission) || !has_fork {
        PushTarget::Upstream
    } else {
        PushTarget::Fork
    }
}

fn is_writable_permission(permission: &str) -> bool {
    matches!(permission, "WRITE" | "MAINTAIN" | "ADMIN")
}

fn cache_age_seconds(checked_at: Option<i64>) -> Option<i64> {
    checked_at.map(|checked| unix_timestamp().saturating_sub(checked))
}

fn cache_state(age_seconds: Option<i64>) -> &'static str {
    match age_seconds {
        Some(age) if age <= PLACEMENT_CACHE_TTL_SECONDS => "fresh",
        Some(_) => "stale",
        None => "missing",
    }
}

fn remote_for_target(topology: &Topology, target: PushTarget) -> Result<String> {
    match target {
        PushTarget::Upstream => topology.canonical_remote.clone().ok_or_else(|| {
            anyhow!(
                "no Git remote pushes to the canonical repository; configure an upstream remote before pushing"
            )
        }),
        PushTarget::Fork => topology.fork_remote.clone().ok_or_else(|| {
            anyhow!("no Git remote pushes to a fork repository; configure a fork remote before pushing")
        }),
        PushTarget::Auto => unreachable!("auto target must be resolved"),
    }
}

fn classify_repo(topology: &Topology, repo: Option<&str>) -> Option<PushTarget> {
    if repo.is_some() && repo == topology.canonical_repo.as_deref() {
        Some(PushTarget::Upstream)
    } else if repo.is_some() && repo == topology.fork_repo.as_deref() {
        Some(PushTarget::Fork)
    } else {
        None
    }
}

fn find_remote(remotes: &[RemoteInfo], repo: &str, preferred: Option<&str>) -> Option<String> {
    remotes
        .iter()
        .filter(|remote| push_repo(remote).as_deref() == Some(repo))
        .min_by_key(|remote| usize::from(Some(remote.name.as_str()) != preferred))
        .map(|remote| remote.name.clone())
}

fn fetch_repo(remote: &RemoteInfo) -> Option<String> {
    remote
        .fetch_url
        .as_deref()
        .and_then(github_repo_slug_from_web_url)
}

fn push_repo(remote: &RemoteInfo) -> Option<String> {
    remote
        .push_url
        .as_deref()
        .and_then(github_repo_slug_from_web_url)
        .or_else(|| fetch_repo(remote))
}

fn repo_for_remote(git: &Git, remote: &str) -> Result<Option<String>> {
    let info = git
        .remote_infos()?
        .into_iter()
        .find(|candidate| candidate.name == remote);
    Ok(info.as_ref().and_then(push_repo))
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_uses_upstream_for_write_permissions() {
        for permission in ["WRITE", "MAINTAIN", "ADMIN"] {
            assert_eq!(
                effective_target(PushTarget::Auto, Some(permission), true),
                PushTarget::Upstream
            );
        }
    }

    #[test]
    fn auto_uses_fork_without_write_permission() {
        for permission in ["READ", "TRIAGE"] {
            assert_eq!(
                effective_target(PushTarget::Auto, Some(permission), true),
                PushTarget::Fork
            );
        }
    }
}
