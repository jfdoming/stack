use std::io::{IsTerminal, stdin, stdout};

use anyhow::{Result, anyhow};
use dialoguer::{Select, theme::ColorfulTheme};

use crate::ui::interaction::{confirm_inline_yes_no, prompt_or_cancel};
use crate::ui::pickers::build_delete_picker_items;
use crate::db::Database;
use crate::git::Git;

pub fn run(
    db: &Database,
    git: &Git,
    branch_arg: Option<&str>,
    porcelain: bool,
    base_branch: &str,
    yes: bool,
) -> Result<()> {
    let current = git.current_branch()?;
    let records = db.list_branches()?;
    let viable_names: Vec<String> = records
        .iter()
        .filter(|r| r.name != base_branch)
        .map(|r| r.name.clone())
        .collect();

    let mut assumed_target = false;
    let branch = if let Some(branch) = branch_arg {
        branch.to_string()
    } else if viable_names.is_empty() {
        base_branch.to_string()
    } else if viable_names.len() == 1 {
        let assumed = viable_names[0].clone();
        if !porcelain {
            println!("assuming target branch '{assumed}' (only viable branch)");
        }
        assumed_target = true;
        assumed
    } else if stdout().is_terminal() && stdin().is_terminal() {
        let theme = ColorfulTheme::default();
        let picker_items = build_delete_picker_items(&viable_names, &current, &records);
        let default_idx = viable_names.iter().position(|b| b == &current).unwrap_or(0);
        let idx = prompt_or_cancel(
            Select::with_theme(&theme)
                .with_prompt(
                    "Select branch to untrack (↑/↓ to navigate, Enter to select, Ctrl-C to cancel)",
                )
                .items(&picker_items)
                .default(default_idx)
                .interact(),
        )?;
        viable_names[idx].clone()
    } else {
        return Err(anyhow!(
            "branch required in non-interactive mode; pass stack untrack <branch>"
        ));
    };

    if assumed_target && !yes {
        if stdout().is_terminal() && stdin().is_terminal() {
            let confirmed =
                confirm_inline_yes_no(&format!("Untrack assumed target branch '{branch}'?"))?;
            if !confirmed {
                if !porcelain {
                    println!("untrack not applied: confirmation declined; no changes made");
                }
                return Ok(());
            }
        } else {
            return Err(anyhow!(
                "target branch was auto-selected as '{}'; rerun with an explicit branch or pass --yes",
                branch
            ));
        }
    }

    if branch == base_branch {
        let payload = serde_json::json!({
            "branch": branch,
            "action": "untrack",
            "status": "noop",
            "reason": "base branch cannot be untracked"
        });
        if porcelain {
            crate::views::print_json(&payload)?;
        } else {
            println!("base branch '{base_branch}' remains tracked as stack root; no changes made");
        }
        return Ok(());
    }

    if db.branch_by_name(&branch)?.is_none() {
        return Err(anyhow!("branch '{}' is not tracked", branch));
    }

    db.splice_out_branch(&branch)?;

    let payload = serde_json::json!({
        "branch": branch,
        "action": "untrack",
        "status": "ok"
    });

    if porcelain {
        crate::views::print_json(&payload)?;
    } else {
        println!("removed '{branch}' from the stack and re-linked its child branches");
    }

    Ok(())
}
