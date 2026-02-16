use std::collections::{HashMap, HashSet};
use std::io::{IsTerminal, stdin, stdout};

use anyhow::{Context, Result, anyhow};
use crossterm::style::Stylize;
use dialoguer::{Input, Select, theme::ColorfulTheme};

use crate::core::rank_parent_candidates;
use crate::db::{BranchRecord, Database};
use crate::git::Git;
use crate::provider::{PrState, Provider};
use crate::ui::interaction::prompt_or_cancel;
use crate::ui::pickers::build_branch_picker_items;
use crate::util::pr_body::{ManagedBranchRef, managed_pr_section, merge_managed_pr_section};
use crate::util::terminal::osc8_hyperlink;

pub fn run(
    db: &Database,
    git: &Git,
    provider: &dyn Provider,
    parent_arg: &Option<String>,
    insert_arg: &Option<String>,
    name_arg: &Option<String>,
    porcelain: bool,
) -> Result<()> {
    let current = git.current_branch()?;
    let tracked = db.list_branches()?;
    let local = git.local_branches()?;
    let parent_candidates = rank_parent_candidates(&current, &tracked, &local);
    let picker_items = build_branch_picker_items(&parent_candidates, &current, &tracked);
    let theme = ColorfulTheme::default();

    let (parent, inserted_before) = if let Some(insert_value) = insert_arg {
        let child = resolve_insert_target(&tracked, git, insert_value, porcelain, &theme)?;
        let child_record = tracked
            .iter()
            .find(|b| b.name == *child)
            .ok_or_else(|| anyhow!("child branch is not tracked: {child}"))?;
        let by_id: HashMap<i64, &BranchRecord> = tracked.iter().map(|b| (b.id, b)).collect();
        let parent = child_record
            .parent_branch_id
            .and_then(|id| by_id.get(&id))
            .map(|b| b.name.clone())
            .ok_or_else(|| anyhow!("cannot insert before '{child}': branch has no parent"))?;
        (parent, Some(child))
    } else {
        let parent = if let Some(parent) = parent_arg {
            parent.clone()
        } else if parent_candidates.len() == 1 {
            let assumed = parent_candidates[0].clone();
            if !porcelain {
                println!("assuming parent branch '{assumed}' (only viable branch)");
            }
            assumed
        } else if stdout().is_terminal() && stdin().is_terminal() {
            let default_idx = parent_candidates
                .iter()
                .position(|b| b == &current)
                .unwrap_or(0);
            let idx = prompt_or_cancel(
                Select::with_theme(&theme)
                    .with_prompt(
                        "Select parent branch (↑/↓ to navigate, Enter to select, Ctrl-C to cancel)",
                    )
                    .items(&picker_items)
                    .default(default_idx)
                    .interact(),
            )?;
            parent_candidates[idx].clone()
        } else {
            return Err(anyhow!(
                "parent required in non-interactive mode; pass --parent <branch>"
            ));
        };
        (parent, None)
    };

    if let Some(before) = inserted_before.as_deref()
        && !git.branch_exists(before)?
    {
        return Err(anyhow!("child branch does not exist in git: {before}"));
    }

    if !git.branch_exists(&parent)? {
        return Err(anyhow!("parent branch does not exist in git: {parent}"));
    }

    let child = if let Some(name) = name_arg {
        name.clone()
    } else if stdout().is_terminal() && stdin().is_terminal() {
        prompt_or_cancel(
            Input::<String>::with_theme(&theme)
                .with_prompt("Name for new child branch")
                .validate_with(|input: &String| -> Result<(), &str> {
                    if input.trim().is_empty() {
                        Err("branch name cannot be empty")
                    } else {
                        Ok(())
                    }
                })
                .interact_text(),
        )?
    } else {
        return Err(anyhow!(
            "branch name required in non-interactive mode; pass --name <branch>"
        ));
    };

    if git.branch_exists(&child)? {
        return Err(anyhow!("branch already exists: {child}"));
    }
    if inserted_before.as_deref() == Some(child.as_str()) {
        return Err(anyhow!(
            "new branch and --insert target cannot be the same: {child}"
        ));
    }

    git.create_branch_from(&child, &parent)
        .with_context(|| format!("failed to create branch {child} from {parent}"))?;
    git.checkout_branch(&child)
        .with_context(|| format!("failed to switch to new branch {child}"))?;

    db.set_parent(&child, Some(&parent))?;
    if let Some(before) = inserted_before.as_deref() {
        db.set_parent(before, Some(&child))?;
    }

    let child_sha = git.head_sha(&child)?;
    let create_url = String::new();
    db.set_sync_sha(&child, &child_sha)?;

    if let Some(before) = inserted_before.as_deref() {
        let base_branch = db.repo_meta()?.base_branch;
        refresh_managed_pr_bodies(
            db,
            git,
            provider,
            &base_branch,
            &[parent.clone(), before.to_string()],
        )?;
    }

    let out = serde_json::json!({
        "created": child,
        "parent": parent,
        "inserted_before": inserted_before,
        "head_sha": child_sha,
        "db": db_summary_path(git)?,
        "create_url": create_url,
    });

    if porcelain {
        crate::views::print_json(&out)?;
    } else {
        let use_color = stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
        if use_color {
            println!(
                "created stack branch: {} -> {}{}",
                out["parent"].as_str().unwrap_or("<unknown>").green().bold(),
                out["created"].as_str().unwrap_or("<unknown>").cyan().bold(),
                if out["create_url"].as_str().unwrap_or_default().is_empty() {
                    String::new()
                } else {
                    format!(
                        " {}",
                        osc8_hyperlink(
                            out["create_url"].as_str().unwrap_or_default(),
                            "open compare",
                        )
                        .dark_grey()
                        .underlined()
                    )
                }
            );
        } else {
            println!(
                "created stack branch: {} -> {}{}",
                out["parent"],
                out["created"],
                if out["create_url"].as_str().unwrap_or_default().is_empty() {
                    String::new()
                } else {
                    format!(" {}", out["create_url"])
                }
            );
        }
    }
    Ok(())
}

fn resolve_insert_target(
    tracked: &[BranchRecord],
    git: &Git,
    insert_value: &str,
    porcelain: bool,
    theme: &ColorfulTheme,
) -> Result<String> {
    if !insert_value.is_empty() {
        return Ok(insert_value.to_string());
    }

    let mut candidates = Vec::new();
    for branch in tracked {
        if branch.parent_branch_id.is_some() && git.branch_exists(&branch.name)? {
            candidates.push(branch.name.clone());
        }
    }
    candidates.sort();

    if candidates.is_empty() {
        return Err(anyhow!(
            "no tracked child branches are available; pass --insert <child>"
        ));
    }
    if candidates.len() == 1 {
        let assumed = candidates[0].clone();
        if !porcelain {
            println!("assuming child branch '{assumed}' (only viable branch)");
        }
        return Ok(assumed);
    }

    if !(stdout().is_terminal() && stdin().is_terminal()) {
        return Err(anyhow!(
            "child required in non-interactive mode; pass --insert <child>"
        ));
    }

    let idx = prompt_or_cancel(
        Select::with_theme(theme)
            .with_prompt("Select child branch to insert before (↑/↓ to navigate, Enter to select, Ctrl-C to cancel)")
            .items(&candidates)
            .default(0)
            .interact(),
    )?;
    Ok(candidates[idx].clone())
}

fn refresh_managed_pr_bodies(
    db: &Database,
    git: &Git,
    provider: &dyn Provider,
    base_branch: &str,
    branches: &[String],
) -> Result<()> {
    let tracked = db.list_branches()?;
    let branch_exists: HashMap<String, bool> = tracked
        .iter()
        .map(|b| Ok((b.name.clone(), git.branch_exists(&b.name)?)))
        .collect::<Result<HashMap<_, _>>>()?;
    let metadata_targets: Vec<(&str, Option<i64>)> = tracked
        .iter()
        .filter(|branch| branch.name != base_branch)
        .filter(|branch| branch_exists.get(&branch.name).copied().unwrap_or(false))
        .map(|branch| (branch.name.as_str(), branch.cached_pr_number))
        .collect();
    let pr_by_branch = provider.resolve_prs_by_head(&metadata_targets)?;

    let fallback_base_url = git
        .remote_web_url("origin")?
        .or_else(|| git.remote_web_url("upstream").ok().flatten());
    let by_id: HashMap<i64, &BranchRecord> = tracked.iter().map(|r| (r.id, r)).collect();
    let mut children: HashMap<i64, Vec<&BranchRecord>> = HashMap::new();
    for branch in &tracked {
        if let Some(parent_id) = branch.parent_branch_id {
            children.entry(parent_id).or_default().push(branch);
        }
    }

    let mut unique_targets = HashSet::new();
    for branch_name in branches {
        if !unique_targets.insert(branch_name.clone()) {
            continue;
        }
        let Some(record) = tracked.iter().find(|r| r.name == *branch_name) else {
            continue;
        };
        let Some(pr) = pr_by_branch.get(branch_name) else {
            continue;
        };
        if !matches!(pr.state, PrState::Open) {
            continue;
        }

        let pr_root = pr
            .url
            .as_deref()
            .and_then(repo_root_from_pr_url)
            .or(fallback_base_url.as_deref())
            .ok_or_else(|| anyhow!("could not determine PR repository URL for '{branch_name}'"))?;
        let parent_ref = record
            .parent_branch_id
            .and_then(|parent_id| by_id.get(&parent_id).copied())
            .map(|parent| ManagedBranchRef {
                branch: parent.name.clone(),
                pr_number: pr_by_branch.get(&parent.name).map(|p| p.number),
                pr_url: pr_by_branch.get(&parent.name).and_then(|p| p.url.clone()),
            });
        let first_child = children.get(&record.id).and_then(|items| {
            items
                .iter()
                .map(|child| ManagedBranchRef {
                    branch: child.name.clone(),
                    pr_number: pr_by_branch.get(&child.name).map(|p| p.number),
                    pr_url: pr_by_branch.get(&child.name).and_then(|p| p.url.clone()),
                })
                .min_by(|a, b| a.branch.cmp(&b.branch))
        });
        let base_commit_url = git
            .merge_base(branch_name, base_branch)
            .ok()
            .map(|sha| format!("{}/commit/{sha}", pr_root.trim_end_matches('/')));
        let managed = managed_pr_section(
            pr_root,
            base_branch,
            base_commit_url.as_deref(),
            parent_ref.as_ref(),
            first_child.as_ref(),
        );
        let merged = merge_managed_pr_section(pr.body.as_deref(), &managed);
        if pr.body.as_deref().map(str::trim) != Some(merged.trim()) {
            provider.update_pr_body(pr.number, &merged)?;
        }
    }

    Ok(())
}

fn repo_root_from_pr_url(url: &str) -> Option<&str> {
    url.split_once("/pull/").map(|(root, _)| root)
}

fn db_summary_path(git: &Git) -> Result<String> {
    Ok(git.git_dir()?.join("stack.db").display().to_string())
}
