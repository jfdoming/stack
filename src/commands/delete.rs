use std::collections::HashMap;
use std::io::{IsTerminal, stdin, stdout};

use anyhow::{Result, anyhow};
use dialoguer::{Select, theme::ColorfulTheme};

use crate::args::DeleteArgs;
use crate::db::{BranchRecord, Database};
use crate::git::{Git, LocalBranchConfig};
use crate::provider::{PrState, Provider};
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
    if target == base_branch {
        return Err(anyhow!("cannot delete base branch '{base_branch}'"));
    }
    let branch = db
        .branch_by_name(&target)?
        .ok_or_else(|| anyhow!("branch '{}' is not tracked", target))?;
    let by_id: HashMap<i64, &BranchRecord> = records.iter().map(|r| (r.id, r)).collect();
    let parent_name = branch
        .parent_branch_id
        .and_then(|id| by_id.get(&id).map(|b| b.name.as_str()))
        .unwrap_or(base_branch)
        .to_string();
    let expected_head = git.head_sha(&branch.name)?;

    let pr = provider.resolve_pr_by_head(&branch.name, branch.cached_pr_number)?;
    let pr_number = pr.as_ref().map(|pr| pr.identity.number);

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
        let pr_label = pr_number
            .map(|number| format!("#{number}"))
            .unwrap_or_else(|| "none".to_string());
        println!(
            "would delete branch '{}' (PR: {}) and splice children under '{}'",
            branch.name, pr_label, parent_name
        );
        return Ok(());
    }

    let should_apply = if yes {
        true
    } else if stdout().is_terminal() && stdin().is_terminal() {
        confirm_inline_yes_no(&format!(
            "Delete '{}' and close its upstream PR?",
            branch.name
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

    let closes_open_pr = pr
        .as_ref()
        .is_some_and(|pr| matches!(pr.state, PrState::Open));
    if !closes_open_pr {
        match pr.as_ref().map(|pr| &pr.state) {
            None => eprintln!("warning: no upstream PR found for '{}'", branch.name),
            Some(PrState::Merged | PrState::Closed) => {}
            Some(PrState::Unknown) => eprintln!(
                "warning: upstream PR state is unknown for '{}'; leaving it unchanged",
                branch.name
            ),
            Some(PrState::Open) => unreachable!("open PRs are closed above"),
        }
    }

    let restore_checkout = current == branch.name;
    if current == branch.name {
        if parent_name == branch.name {
            return Err(anyhow!(
                "cannot delete current branch '{}' without switching branches",
                branch.name
            ));
        }
        git.checkout_branch(&parent_name)?;
    }

    let branch_config = match git.remove_local_branch_config(&branch.name) {
        Ok(config) => config,
        Err(config_error) => {
            let restore_result = if restore_checkout {
                git.checkout_branch(&branch.name)
            } else {
                Ok(())
            };
            return match restore_result {
                Ok(()) => Err(config_error),
                Err(restore_error) => Err(anyhow!(
                    "{config_error}; additionally failed to restore checkout of '{}': {restore_error}",
                    branch.name
                )),
            };
        }
    };

    if let Err(delete_error) =
        git.delete_local_branch_ref_if_unchanged(&branch.name, &expected_head)
    {
        let restore_result =
            restore_local_delete_state(git, &branch.name, None, &branch_config, restore_checkout);
        return match restore_result {
            Ok(()) => Err(delete_error),
            Err(restore_error) => Err(anyhow!(
                "{delete_error}; additionally failed to restore local branch '{}': {restore_error}",
                branch.name
            )),
        };
    }
    if let Err(splice_error) = db.splice_out_branch_if_unchanged(&branch) {
        let restore_result = restore_local_delete_state(
            git,
            &branch.name,
            Some(&expected_head),
            &branch_config,
            restore_checkout,
        );
        return match restore_result {
            Ok(()) => Err(splice_error),
            Err(restore_error) => Err(anyhow!(
                "{splice_error}; additionally failed to restore local branch '{}': {restore_error}",
                branch.name
            )),
        };
    }

    if let Some(pr) = pr.as_ref()
        && closes_open_pr
        && let Err(close_error) = provider.close_pr(&pr.identity)
    {
        return Err(anyhow!(
            "deleted local branch '{}' and spliced stack metadata, but could not confirm closure of PR #{} in '{}': {close_error}; inspect the PR and close it manually if it remains open",
            branch.name,
            pr.identity.number,
            pr.identity.repository
        ));
    }

    let closed_pr_number = if closes_open_pr { pr_number } else { None };
    if porcelain {
        return crate::views::print_json(&serde_json::json!({
            "deleted_branch": branch.name,
            "closed_pr_number": closed_pr_number,
            "spliced_to_parent": parent_name,
        }));
    }
    println!(
        "deleted '{}' and spliced stack children to '{}'",
        branch.name, parent_name
    );
    Ok(())
}

fn restore_local_delete_state(
    git: &Git,
    branch: &str,
    deleted_head: Option<&str>,
    config: &LocalBranchConfig,
    restore_checkout: bool,
) -> Result<()> {
    let mut errors = Vec::new();
    if let Some(expected_head) = deleted_head
        && let Err(error) = git.restore_local_branch_ref_if_missing(branch, expected_head)
    {
        errors.push(format!("ref: {error}"));
    }
    if let Err(error) = git.restore_local_branch_config(config) {
        errors.push(format!("configuration: {error}"));
    }
    if restore_checkout && let Err(error) = git.checkout_branch(branch) {
        errors.push(format!("checkout: {error}"));
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(errors.join("; ")))
    }
}
