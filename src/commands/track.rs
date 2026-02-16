use std::collections::{HashMap, HashSet};
use std::io::{IsTerminal, stdin, stdout};

use anyhow::{Result, anyhow};
use dialoguer::{Select, theme::ColorfulTheme};

use crate::args::TrackArgs;
use crate::core::rank_parent_candidates;
use crate::db::{BranchRecord, Database, ParentUpdate};
use crate::git::Git;
use crate::provider::Provider;
use crate::ui::interaction::{UserCancelled, confirm_inline_yes_no, prompt_or_cancel};
use crate::ui::pickers::build_branch_picker_items;

#[derive(Debug, Clone)]
pub struct TrackRunOptions {
    pub porcelain: bool,
    pub yes: bool,
    pub dry_run: bool,
    pub force: bool,
    pub debug: bool,
}

#[derive(Debug, Clone, Copy)]
enum TrackSource {
    Explicit,
    PrBase,
    GitAncestry,
}

impl TrackSource {
    fn as_str(self) -> &'static str {
        match self {
            TrackSource::Explicit => "explicit",
            TrackSource::PrBase => "pr_base",
            TrackSource::GitAncestry => "git_ancestry",
        }
    }
}

#[derive(Debug, Clone)]
struct ParentInference {
    parent: String,
    source: TrackSource,
    confidence: &'static str,
}

#[derive(Debug, Clone)]
struct TrackChange {
    branch: String,
    old_parent: Option<String>,
    new_parent: String,
    source: TrackSource,
    confidence: &'static str,
}

#[derive(Debug, Clone)]
struct TrackSkip {
    branch: String,
    reason: String,
}

pub fn run(
    db: &Database,
    git: &Git,
    provider: &dyn Provider,
    args: &TrackArgs,
    base_branch: &str,
    opts: TrackRunOptions,
) -> Result<()> {
    if args.all && args.branch.is_some() {
        return Err(anyhow!(
            "cannot combine --all with a positional branch argument"
        ));
    }
    if args.all && args.parent.is_some() {
        return Err(anyhow!("cannot combine --all with --parent"));
    }

    let is_tty = stdout().is_terminal() && stdin().is_terminal();
    let current = git.current_branch()?;
    let tracked = db.list_branches()?;
    let by_name: HashMap<String, BranchRecord> = tracked
        .iter()
        .map(|b| (b.name.clone(), b.clone()))
        .collect();
    let by_id: HashMap<i64, String> = tracked.iter().map(|b| (b.id, b.name.clone())).collect();
    let local = git.local_branches()?;
    let local_set: HashSet<String> = local.iter().cloned().collect();
    let mut changes = Vec::new();
    let mut skipped = Vec::new();
    let mut unresolved = Vec::new();
    let mut warnings = Vec::new();

    let mut assumed_target: Option<String> = None;
    let targets: Vec<String> = if args.all {
        local
            .iter()
            .filter(|b| b.as_str() != base_branch)
            .cloned()
            .collect()
    } else if let Some(branch) = &args.branch {
        vec![branch.clone()]
    } else {
        let viable_names: Vec<String> = local
            .iter()
            .filter(|b| b.as_str() != base_branch)
            .cloned()
            .collect();
        if viable_names.is_empty() {
            return Err(anyhow!("no local non-base branches available to track"));
        }
        if viable_names.len() == 1 {
            let assumed = viable_names[0].clone();
            if !opts.porcelain {
                println!("assuming target branch '{assumed}' (only viable branch)");
            }
            assumed_target = Some(assumed.clone());
            vec![assumed]
        } else if is_tty {
            let theme = ColorfulTheme::default();
            let picker_items = build_branch_picker_items(&viable_names, &current, &tracked);
            let default_idx = viable_names.iter().position(|b| b == &current).unwrap_or(0);
            let idx = prompt_or_cancel(
                Select::with_theme(&theme)
                    .with_prompt(
                        "Select branch to track (↑/↓ to navigate, Enter to select, Ctrl-C to cancel)",
                    )
                    .items(&picker_items)
                    .default(default_idx)
                    .interact(),
            )?;
            vec![viable_names[idx].clone()]
        } else {
            return Err(anyhow!(
                "branch required in non-interactive mode; pass stack track <branch>"
            ));
        }
    };

    if let Some(assumed) = &assumed_target
        && !opts.yes
        && !opts.dry_run
    {
        if is_tty {
            let confirmed =
                confirm_inline_yes_no(&format!("Track assumed target branch '{assumed}'?"))?;
            if !confirmed {
                if !opts.porcelain {
                    println!("track not applied: confirmation declined; no changes made");
                }
                return Ok(());
            }
        } else {
            return Err(anyhow!(
                "target branch was auto-selected as '{}'; rerun with an explicit branch or pass --yes",
                assumed
            ));
        }
    }

    for target in targets {
        if !local_set.contains(&target) {
            return Err(anyhow!("branch '{}' does not exist in git", target));
        }
        if target == base_branch {
            skipped.push(TrackSkip {
                branch: target,
                reason: "base branch is not eligible for tracking".to_string(),
            });
            continue;
        }

        let inference = if args.all {
            infer_parent_for_branch(
                git,
                provider,
                &target,
                by_name.get(&target),
                &local,
                &mut warnings,
                opts.debug,
            )?
        } else if let Some(parent) = &args.parent {
            if !local_set.contains(parent) {
                return Err(anyhow!("parent branch does not exist in git: {}", parent));
            }
            Some(ParentInference {
                parent: parent.clone(),
                source: TrackSource::Explicit,
                confidence: "high",
            })
        } else {
            let inferred = infer_parent_for_branch(
                git,
                provider,
                &target,
                by_name.get(&target),
                &local,
                &mut warnings,
                opts.debug,
            )?;
            if inferred.is_some() || args.infer {
                inferred
            } else {
                let parent_candidates: Vec<String> =
                    rank_parent_candidates(&target, &tracked, &local)
                        .into_iter()
                        .filter(|candidate| candidate != &target)
                        .collect();
                if parent_candidates.is_empty() {
                    return Err(anyhow!(
                        "no viable parent branches available for '{}'",
                        target
                    ));
                }
                let parent = if parent_candidates.len() == 1 {
                    let assumed = parent_candidates[0].clone();
                    if !opts.porcelain {
                        println!("assuming parent branch '{assumed}' (only viable branch)");
                    }
                    assumed
                } else if is_tty {
                    let theme = ColorfulTheme::default();
                    let picker_items =
                        build_branch_picker_items(&parent_candidates, &current, &tracked);
                    let default_idx = parent_candidates
                        .iter()
                        .position(|b| b == &current)
                        .unwrap_or(0);
                    let idx = prompt_or_cancel(
                        Select::with_theme(&theme)
                            .with_prompt(format!(
                                "Select parent branch for '{}' (↑/↓ to navigate, Enter to select, Ctrl-C to cancel)",
                                target
                            ))
                            .items(&picker_items)
                            .default(default_idx)
                            .interact(),
                    )?;
                    parent_candidates[idx].clone()
                } else {
                    return Err(anyhow!(
                        "could not infer a parent in non-interactive mode; pass --parent <branch> or use --infer to allow unresolved output"
                    ));
                };
                Some(ParentInference {
                    parent,
                    source: TrackSource::Explicit,
                    confidence: "high",
                })
            }
        };

        let Some(parent) = inference else {
            unresolved.push(target);
            continue;
        };

        if parent.parent == target {
            unresolved.push(target);
            continue;
        }
        if !local_set.contains(&parent.parent) {
            return Err(anyhow!(
                "inferred parent branch does not exist in git: {}",
                parent.parent
            ));
        }

        let old_parent = by_name
            .get(&target)
            .and_then(|rec| rec.parent_branch_id)
            .and_then(|id| by_id.get(&id).cloned());
        if old_parent.as_deref() == Some(parent.parent.as_str()) {
            skipped.push(TrackSkip {
                branch: target,
                reason: "already linked to inferred parent".to_string(),
            });
            continue;
        }

        changes.push(TrackChange {
            branch: target,
            old_parent,
            new_parent: parent.parent,
            source: parent.source,
            confidence: parent.confidence,
        });
    }

    let mut apply_changes = Vec::new();
    for change in changes {
        if change.old_parent.is_some() && change.old_parent.as_deref() != Some(&change.new_parent) {
            if opts.yes {
                apply_changes.push(change);
                continue;
            }
            if !is_tty {
                if !opts.force {
                    return Err(anyhow!(
                        "parent conflict for '{}': existing '{}' and proposed '{}' (use --force in non-interactive mode)",
                        change.branch,
                        change.old_parent.as_deref().unwrap_or("<none>"),
                        change.new_parent
                    ));
                }
                apply_changes.push(change);
                continue;
            }

            match prompt_track_conflict(&change)? {
                TrackConflictResolution::Replace => apply_changes.push(change),
                TrackConflictResolution::Skip => skipped.push(TrackSkip {
                    branch: change.branch,
                    reason: "conflict skipped by user".to_string(),
                }),
                TrackConflictResolution::Abort => return Err(UserCancelled.into()),
            }
        } else {
            apply_changes.push(change);
        }
    }

    let applied = !opts.dry_run && !apply_changes.is_empty();
    if applied {
        let updates: Vec<ParentUpdate> = apply_changes
            .iter()
            .map(|c| ParentUpdate {
                child_name: c.branch.clone(),
                parent_name: Some(c.new_parent.clone()),
            })
            .collect();
        db.set_parents_batch(&updates)?;
    }

    let changes_payload: Vec<serde_json::Value> = apply_changes
        .iter()
        .map(|c| {
            serde_json::json!({
                "branch": c.branch,
                "old_parent": c.old_parent,
                "new_parent": c.new_parent,
                "source": c.source.as_str(),
                "confidence": c.confidence,
            })
        })
        .collect();
    let skipped_payload: Vec<serde_json::Value> = skipped
        .iter()
        .map(|s| serde_json::json!({"branch": s.branch, "reason": s.reason}))
        .collect();

    let payload = serde_json::json!({
        "mode": if args.all { "all" } else { "single" },
        "dry_run": opts.dry_run,
        "applied": applied,
        "changes": changes_payload,
        "skipped": skipped_payload,
        "unresolved": unresolved,
        "warnings": warnings,
    });

    if opts.porcelain {
        crate::views::print_json(&payload)?;
        if args.all && !opts.dry_run && !is_tty && !unresolved.is_empty() {
            return Err(anyhow!("some branches could not be resolved"));
        }
        return Ok(());
    }

    for change in &apply_changes {
        println!(
            "{} '{}' under '{}' (source: {}, confidence: {})",
            if opts.dry_run {
                "would track"
            } else {
                "tracking"
            },
            change.branch,
            change.new_parent,
            change.source.as_str(),
            change.confidence
        );
    }
    for skip in skipped {
        println!("skipped '{}': {}", skip.branch, skip.reason);
    }
    for branch in &unresolved {
        println!("could not determine a parent for '{}'", branch);
    }
    for warning in &warnings {
        eprintln!("warning: {warning}");
    }

    if opts.dry_run {
        println!("track dry run complete; no changes were made");
    } else if applied {
        println!("tracking updated");
    } else {
        println!("no tracking changes were needed");
    }

    if args.all && !opts.dry_run && !is_tty && !unresolved.is_empty() {
        return Err(anyhow!("some branches could not be resolved"));
    }
    Ok(())
}

fn infer_parent_for_branch(
    git: &Git,
    provider: &dyn Provider,
    branch: &str,
    tracked: Option<&BranchRecord>,
    local: &[String],
    warnings: &mut Vec<String>,
    debug: bool,
) -> Result<Option<ParentInference>> {
    let cached_number = tracked.and_then(|r| r.cached_pr_number);
    match provider.resolve_pr_by_head(branch, cached_number) {
        Ok(Some(pr)) => {
            if let Some(base) = pr.base_ref_name
                && base != branch
                && git.branch_exists(&base)?
            {
                return Ok(Some(ParentInference {
                    parent: base,
                    source: TrackSource::PrBase,
                    confidence: "high",
                }));
            }
        }
        Ok(None) => {}
        Err(err) => warnings.push(format_pr_metadata_warning(branch, &err, debug)),
    }

    infer_parent_from_git(git, branch, local)
}

fn format_pr_metadata_warning(branch: &str, err: &anyhow::Error, debug: bool) -> String {
    let raw = err.to_string();
    if debug {
        return format!(
            "could not read PR metadata for '{}'; falling back to git ancestry ({})",
            branch, raw
        );
    }
    if raw.contains("expected value at line 1 column 1")
        || raw.contains("EOF while parsing")
        || raw.contains("trailing characters")
    {
        return format!(
            "could not read PR metadata for '{}'; gh returned an unexpected response. Falling back to git ancestry.",
            branch
        );
    }
    format!(
        "could not read PR metadata for '{}'; falling back to git ancestry ({})",
        branch, raw
    )
}

fn infer_parent_from_git(
    git: &Git,
    branch: &str,
    local: &[String],
) -> Result<Option<ParentInference>> {
    let mut best_parent: Option<String> = None;
    let mut best_distance = u32::MAX;
    let mut tied = false;
    for candidate in local {
        if candidate == branch {
            continue;
        }
        if !git.is_ancestor(candidate, branch)? {
            continue;
        }
        let distance = git.commit_distance(candidate, branch)?;
        if distance < best_distance {
            best_parent = Some(candidate.clone());
            best_distance = distance;
            tied = false;
        } else if distance == best_distance {
            tied = true;
        }
    }

    if tied {
        return Ok(None);
    }
    Ok(best_parent.map(|parent| ParentInference {
        parent,
        source: TrackSource::GitAncestry,
        confidence: "medium",
    }))
}

enum TrackConflictResolution {
    Replace,
    Skip,
    Abort,
}

fn prompt_track_conflict(change: &TrackChange) -> Result<TrackConflictResolution> {
    let theme = ColorfulTheme::default();
    let items = vec![
        "Replace parent".to_string(),
        "Skip branch".to_string(),
        "Abort".to_string(),
    ];
    let old = change.old_parent.as_deref().unwrap_or("<none>");
    let idx = prompt_or_cancel(
        Select::with_theme(&theme)
            .with_prompt(format!(
                "Parent conflict for '{}' (existing: '{}', proposed: '{}')",
                change.branch, old, change.new_parent
            ))
            .items(&items)
            .default(0)
            .interact(),
    )?;
    Ok(match idx {
        0 => TrackConflictResolution::Replace,
        1 => TrackConflictResolution::Skip,
        _ => TrackConflictResolution::Abort,
    })
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;

    use super::format_pr_metadata_warning;

    #[test]
    fn pr_metadata_parse_error_warning_is_user_friendly() {
        let err = anyhow!("expected value at line 1 column 1");
        let msg = format_pr_metadata_warning("feat/a", &err, false);
        assert!(msg.contains("gh returned an unexpected response"));
        assert!(!msg.contains("line 1 column 1"));
    }
}
