use std::collections::HashMap;
use std::io::{IsTerminal, stdin, stdout};

use anyhow::{Result, anyhow};
use dialoguer::{Select, theme::ColorfulTheme};

use crate::args::DeleteArgs;
use crate::db::{BranchRecord, Database};
use crate::git::Git;
use crate::provider::Provider;
use crate::ui::interaction::{confirm_inline_yes_no, prompt_or_cancel};
use crate::ui::pickers::build_delete_picker_items;

pub fn run(
    db: &Database,
    git: &Git,
    provider: &dyn Provider,
    args: &DeleteArgs,
    porcelain: bool,
    yes: bool,
    base_branch: &str,
) -> Result<()> {
    let current = git.current_branch()?;
    let records = db.list_branches()?;
    let viable_names: Vec<String> = records
        .iter()
        .filter(|r| r.name != base_branch)
        .map(|r| r.name.clone())
        .collect();
    let theme = ColorfulTheme::default();

    if args.branch.is_none() && viable_names.is_empty() {
        return Err(anyhow!("no tracked non-base branches available to delete"));
    }

    let target = if let Some(branch) = &args.branch {
        branch.clone()
    } else if viable_names.len() == 1 {
        let assumed = viable_names[0].clone();
        if !porcelain {
            println!("assuming target branch '{assumed}' (only viable branch)");
        }
        assumed
    } else if stdout().is_terminal() && stdin().is_terminal() {
        let picker_items = build_delete_picker_items(&viable_names, &current, &records);
        let default_idx = viable_names.iter().position(|b| b == &current).unwrap_or(0);
        let idx = prompt_or_cancel(
            Select::with_theme(&theme)
                .with_prompt(
                    "Select branch to delete (↑/↓ to navigate, Enter to select, Ctrl-C to cancel)",
                )
                .items(&picker_items)
                .default(default_idx)
                .interact(),
        )?;
        viable_names[idx].clone()
    } else {
        return Err(anyhow!(
            "branch required in non-interactive mode; pass stack delete <branch>"
        ));
    };
    let branch = db
        .branch_by_name(&target)?
        .ok_or_else(|| anyhow!("branch '{}' is not tracked", target))?;
    let by_id: HashMap<i64, &BranchRecord> = records.iter().map(|r| (r.id, r)).collect();
    let parent_name = branch
        .parent_branch_id
        .and_then(|id| by_id.get(&id).map(|b| b.name.as_str()))
        .unwrap_or(base_branch)
        .to_string();

    let mut pr_number = branch.cached_pr_number;
    if pr_number.is_none()
        && let Some(pr) = provider.resolve_pr_by_head(&branch.name, None)?
    {
        pr_number = Some(pr.number);
    }

    let payload = serde_json::json!({
        "branch": branch.name,
        "parent": parent_name,
        "pr_number": pr_number,
        "dry_run": args.dry_run,
    });

    if args.dry_run {
        if porcelain {
            return crate::views::print_json(&payload);
        }
        println!(
            "would delete branch '{}' (PR: {:?}) and splice children under '{}'",
            payload["branch"], payload["pr_number"], payload["parent"]
        );
        return Ok(());
    }

    let should_apply = if yes {
        true
    } else if stdout().is_terminal() && stdin().is_terminal() {
        confirm_inline_yes_no(&format!(
            "Delete '{}' and close its upstream PR?",
            payload["branch"]
        ))?
    } else {
        false
    };
    if !should_apply {
        if !porcelain {
            println!("delete not applied: confirmation declined; no changes made");
        }
        return Ok(());
    }

    if let Some(number) = pr_number {
        provider.delete_pr(number)?;
    } else {
        eprintln!("warning: no upstream PR found for '{}'", branch.name);
    }

    if current == branch.name {
        if parent_name == branch.name {
            return Err(anyhow!(
                "cannot delete current branch '{}' without switching branches",
                branch.name
            ));
        }
        git.checkout_branch(&parent_name)?;
    }

    git.delete_local_branch(&branch.name)?;
    db.splice_out_branch(&branch.name)?;

    if porcelain {
        return crate::views::print_json(&serde_json::json!({
            "deleted_branch": branch.name,
            "closed_pr_number": pr_number,
            "spliced_to_parent": parent_name,
        }));
    }
    println!(
        "deleted '{}' and spliced stack children to '{}'",
        branch.name, parent_name
    );
    Ok(())
}
