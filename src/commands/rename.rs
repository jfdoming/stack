use std::io::{IsTerminal, stdin, stdout};

use anyhow::{Context, Result, anyhow};
use dialoguer::{Input, Select, theme::ColorfulTheme};

use crate::args::RenameArgs;
use crate::db::BranchRecord;
use crate::db::Database;
use crate::git::Git;
use crate::provider::{PrState, Provider};
use crate::ui::interaction::{confirm_inline_yes_no, prompt_or_cancel};
use crate::ui::pickers::build_branch_picker_items;

pub fn run(
    db: &Database,
    git: &Git,
    provider: &dyn Provider,
    args: &RenameArgs,
    porcelain: bool,
    yes: bool,
    base_branch: &str,
) -> Result<()> {
    let is_tty = stdout().is_terminal() && stdin().is_terminal();
    let current = git.current_branch()?;
    let tracked = db.list_branches()?;
    let (old_branch, new_branch) =
        resolve_branches(args, is_tty, porcelain, base_branch, &current, &tracked)?;

    if old_branch == base_branch {
        return Err(anyhow!("cannot rename base branch '{base_branch}'"));
    }
    if old_branch == new_branch {
        return Err(anyhow!("old and new branch names are identical"));
    }

    let tracked = db
        .branch_by_name(&old_branch)?
        .ok_or_else(|| anyhow!("branch '{}' is not tracked", old_branch))?;
    if db.branch_by_name(&new_branch)?.is_some() {
        return Err(anyhow!("new branch is already tracked: {new_branch}"));
    }
    if !git.branch_exists(&old_branch)? {
        return Err(anyhow!("source branch does not exist in git: {old_branch}"));
    }
    if git.branch_exists(&new_branch)? {
        return Err(anyhow!("new branch already exists in git: {new_branch}"));
    }

    let upstream = git.branch_upstream(&old_branch)?;
    let has_upstream = upstream.is_some();
    let remote = upstream
        .as_deref()
        .and_then(|u| u.split_once('/').map(|(name, _)| name.to_string()))
        .or(git.remote_for_branch(&old_branch)?)
        .unwrap_or_else(|| "origin".to_string());

    let mut open_pr_number = None;
    match provider.resolve_pr_by_head(&old_branch, tracked.cached_pr_number) {
        Ok(Some(pr)) if matches!(pr.state, PrState::Open) => {
            open_pr_number = Some(pr.number);
        }
        Ok(_) => {}
        Err(err) => {
            eprintln!(
                "warning: could not confirm upstream PR state for '{}': {err}",
                old_branch
            );
        }
    }

    if has_upstream && open_pr_number.is_some() {
        eprintln!(
            "warning: deleting the upstream branch may close the open PR for '{}'; this operation will push '{}' and delete '{}' on remote '{}'.",
            old_branch, new_branch, old_branch, remote
        );

        if !yes {
            if is_tty {
                let confirmed = confirm_inline_yes_no(
                    "Continue with upstream branch deletion that may close the PR?",
                )?;
                if !confirmed {
                    if !porcelain {
                        println!("rename not applied: confirmation declined; no changes made");
                    }
                    return Ok(());
                }
            } else {
                return Err(anyhow!(
                    "open PR detected for '{}'; deleting upstream branch requires --yes in non-interactive mode",
                    old_branch
                ));
            }
        }
    }

    let payload = serde_json::json!({
        "old_branch": old_branch,
        "new_branch": new_branch,
        "remote": if has_upstream { serde_json::Value::String(remote.clone()) } else { serde_json::Value::Null },
        "open_pr_detected": open_pr_number.is_some(),
        "dry_run": args.dry_run,
        "applied": !args.dry_run,
        "remote_updated": has_upstream && !args.dry_run,
    });

    if args.dry_run {
        if porcelain {
            return crate::views::print_json(&payload);
        }
        if has_upstream {
            println!(
                "would rename '{}' -> '{}' and update remote '{}' (push new, delete old)",
                old_branch, new_branch, remote
            );
        } else {
            println!(
                "would rename '{}' -> '{}' and skip remote update (source branch has no upstream)",
                old_branch, new_branch
            );
        }
        return Ok(());
    }

    git.rename_local_branch(&old_branch, &new_branch)
        .with_context(|| {
            format!(
                "failed to rename branch '{}' to '{}'",
                old_branch, new_branch
            )
        })?;
    db.rename_branch(&old_branch, &new_branch)
        .with_context(|| format!("failed to update stack metadata for '{}'", old_branch))?;
    db.set_pr_cache(&new_branch, None, None)?;

    if has_upstream {
        git.push_branch(&remote, &new_branch)
            .with_context(|| format!("failed to push renamed branch '{}'", new_branch))?;
        git.delete_remote_branch(&remote, &old_branch)
            .with_context(|| format!("failed to delete old remote branch '{}'", old_branch))?;
    }

    if porcelain {
        return crate::views::print_json(&payload);
    }

    println!("renamed '{}' to '{}'", old_branch, new_branch);
    if has_upstream {
        println!(
            "updated remote '{}': pushed '{}' and deleted '{}'",
            remote, new_branch, old_branch
        );
    } else {
        println!("skipped remote update (source branch has no upstream)");
    }

    Ok(())
}

fn resolve_branches(
    args: &RenameArgs,
    is_tty: bool,
    porcelain: bool,
    base_branch: &str,
    current: &str,
    tracked: &[BranchRecord],
) -> Result<(String, String)> {
    let theme = ColorfulTheme::default();

    let old = if let Some(old) = &args.old {
        old.trim().to_string()
    } else {
        let candidates: Vec<String> = tracked
            .iter()
            .filter(|branch| branch.name != base_branch)
            .map(|branch| branch.name.clone())
            .collect();
        if candidates.is_empty() {
            return Err(anyhow!("no tracked non-base branches available to rename"));
        }
        if candidates.len() == 1 {
            let assumed = candidates[0].clone();
            if !porcelain {
                println!("assuming source branch '{assumed}' (only viable branch)");
            }
            assumed
        } else if is_tty {
            let picker_items = build_branch_picker_items(&candidates, current, tracked);
            let default_idx = candidates.iter().position(|b| b == current).unwrap_or(0);
            let idx = prompt_or_cancel(
                Select::with_theme(&theme)
                    .with_prompt(
                        "Select branch to rename (↑/↓ to navigate, Enter to select, Ctrl-C to cancel)",
                    )
                    .items(&picker_items)
                    .default(default_idx)
                    .interact(),
            )?;
            candidates[idx].clone()
        } else {
            return Err(anyhow!(
                "missing source branch; usage: stack rename <old> <new>"
            ));
        }
    };

    let new = if let Some(new) = &args.new {
        new.trim().to_string()
    } else if is_tty {
        prompt_or_cancel(
            Input::<String>::with_theme(&theme)
                .with_prompt("New branch name")
                .validate_with(|input: &String| -> Result<(), &str> {
                    if input.trim().is_empty() {
                        Err("branch name cannot be empty")
                    } else {
                        Ok(())
                    }
                })
                .interact_text(),
        )?
        .trim()
        .to_string()
    } else {
        return Err(anyhow!(
            "missing new branch; usage: stack rename <old> <new>"
        ));
    };

    if old.is_empty() || new.is_empty() {
        return Err(anyhow!("branch names cannot be empty"));
    }

    Ok((old, new))
}
