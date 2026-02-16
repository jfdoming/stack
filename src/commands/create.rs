use std::io::{IsTerminal, stdin, stdout};

use anyhow::{Context, Result, anyhow};
use crossterm::style::Stylize;
use dialoguer::{Input, Select, theme::ColorfulTheme};

use crate::core::rank_parent_candidates;
use crate::db::Database;
use crate::git::Git;
use crate::ui::interaction::prompt_or_cancel;
use crate::ui::pickers::build_branch_picker_items;
use crate::util::terminal::osc8_hyperlink;

pub fn run(
    db: &Database,
    git: &Git,
    parent_arg: &Option<String>,
    name_arg: &Option<String>,
    porcelain: bool,
) -> Result<()> {
    let current = git.current_branch()?;
    let tracked = db.list_branches()?;
    let local = git.local_branches()?;
    let parent_candidates = rank_parent_candidates(&current, &tracked, &local);
    let picker_items = build_branch_picker_items(&parent_candidates, &current, &tracked);
    let theme = ColorfulTheme::default();

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

    git.create_branch_from(&child, &parent)
        .with_context(|| format!("failed to create branch {child} from {parent}"))?;
    git.checkout_branch(&child)
        .with_context(|| format!("failed to switch to new branch {child}"))?;

    db.set_parent(&child, Some(&parent))?;
    let child_sha = git.head_sha(&child)?;
    let create_url = String::new();
    db.set_sync_sha(&child, &child_sha)?;
    let out = serde_json::json!({
        "created": child,
        "parent": parent,
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

fn db_summary_path(git: &Git) -> Result<String> {
    Ok(git.git_dir()?.join("stack.db").display().to_string())
}
