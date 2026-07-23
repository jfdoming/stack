use anyhow::Result;

use crate::db::Database;
use crate::git::Git;
use crate::provider::Provider;
use crate::{args::PushArgs, core::PushTarget};

pub fn run(
    db: &Database,
    git: &Git,
    provider: &dyn Provider,
    args: &PushArgs,
    porcelain: bool,
    yes: bool,
    base_branch: &str,
) -> Result<()> {
    let records = db.list_branches()?;
    let mut branches: Vec<_> = records
        .iter()
        .filter(|record| record.name != base_branch)
        .collect();
    branches.sort_by(|a, b| a.name.cmp(&b.name));
    branches.dedup_by(|a, b| a.name == b.name);

    let mut skipped_missing = Vec::new();
    let mut skipped_merged = Vec::new();
    let mut pushable = Vec::new();

    for record in branches {
        let branch = record.name.clone();
        if !git.branch_exists(&branch)? {
            skipped_missing.push(branch);
            continue;
        }
        let current_head = git.head_sha(&branch)?;
        let is_merged = record
            .cached_pr_state
            .as_deref()
            .is_some_and(|state| state.eq_ignore_ascii_case("merged"))
            && if let Some(head_oid) = record.cached_pr_head_oid.as_deref() {
                git.is_first_parent_ancestor(head_oid, &current_head)?
            } else {
                false
            };
        if is_merged {
            skipped_merged.push(branch);
            continue;
        }

        pushable.push(branch);
    }

    let requested = args.push_target.map(|target| match target.as_str() {
        "upstream" => PushTarget::Upstream,
        "fork" => PushTarget::Fork,
        _ => PushTarget::Auto,
    });
    let placements = crate::core::resolve_push_placements(
        db,
        git,
        provider,
        crate::core::PlacementRequest {
            records: &records,
            branches: &pushable,
            base_branch,
            requested,
            yes,
        },
    )?;
    let mut pushed = Vec::new();
    for placement in placements {
        if let Err(err) = git.push_branch_force_with_lease(&placement.remote, &placement.branch) {
            if placement.push_target == PushTarget::Upstream {
                db.clear_placement_cache()?;
            }
            return Err(err.context(format!(
                "failed to push '{}' to {}; no fork fallback was attempted. Review the repository policy with `stack config push-target` or select the fork with `stack config push-target fork`",
                placement.branch, placement.remote
            )));
        }
        pushed.push(placement);
    }

    if porcelain {
        let pushed = pushed
            .iter()
            .map(|placement| {
                serde_json::json!({
                    "branch": placement.branch,
                    "remote": placement.remote,
                    "repository": placement.repository,
                    "push_target": placement.push_target,
                    "decision_source": placement.decision_source,
                    "canonical_repository": placement.canonical_repository,
                    "canonical_remote": placement.canonical_remote,
                    "fork_repository": placement.fork_repository,
                    "push_permission": placement.push_permission,
                    "permission_checked_at": placement.permission_checked_at,
                    "cache_age_seconds": placement.cache_age_seconds,
                    "cache_state": placement.cache_state,
                })
            })
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
        for placement in &pushed {
            println!("pushed '{}' to '{}'", placement.branch, placement.remote);
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
