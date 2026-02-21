use std::io::{IsTerminal, stdin, stdout};

use anyhow::Result;
use crossterm::style::Stylize;

use crate::core::build_sync_plan;
use crate::db::Database;
use crate::git::Git;
use crate::provider::Provider;
use crate::ui::interaction::confirm_inline_yes_no;

pub struct SyncRunOptions {
    pub porcelain: bool,
    pub yes: bool,
    pub dry_run: bool,
}

pub fn run(
    db: &Database,
    git: &Git,
    provider: &dyn Provider,
    base_branch: &str,
    base_remote: &str,
    opts: SyncRunOptions,
) -> Result<()> {
    let plan = build_sync_plan(db, git, provider, base_branch, base_remote)?;
    let plan_view = plan.to_view();

    if opts.porcelain {
        crate::views::print_json(&plan_view)?;
    } else {
        println!("sync base: {}", plan.base_branch);
        let use_color = stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
        for op in &plan_view.operations {
            if use_color {
                let kind = match op.kind.as_str() {
                    "fetch" => op.kind.as_str().blue().bold().to_string(),
                    "restack" => op.kind.as_str().yellow().bold().to_string(),
                    "update_sha" => op.kind.as_str().cyan().to_string(),
                    _ => op.kind.clone(),
                };
                println!("- {}: {} {}", kind, op.branch.as_str().green(), op.details);
            } else {
                println!("- {}: {} {}", op.kind, op.branch, op.details);
            }
        }
    }

    if opts.dry_run {
        return Ok(());
    }

    let should_apply = if opts.yes {
        true
    } else if stdout().is_terminal() && stdin().is_terminal() {
        confirm_inline_yes_no("Apply sync plan?")?
    } else {
        false
    };

    if !should_apply {
        if !opts.porcelain {
            println!("sync plan not applied");
        }
        return Ok(());
    }

    crate::core::execute_sync_plan(db, git, provider, &plan)?;
    if !opts.porcelain {
        println!("sync completed");
    }

    if opts.porcelain {
        return Ok(());
    }

    let is_tty = stdout().is_terminal() && stdin().is_terminal();
    let should_push = if !is_tty {
        false
    } else if opts.yes {
        true
    } else {
        confirm_inline_yes_no("Push tracked branches now?")?
    };

    if should_push {
        crate::commands::push::run(db, git, false, base_branch)?;
    }

    Ok(())
}
