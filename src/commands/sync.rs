use std::io::{IsTerminal, stdin, stdout};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::style::Stylize;

use crate::core::{SyncPlanTiming, build_sync_plan};
use crate::db::Database;
use crate::git::Git;
use crate::provider::Provider;
use crate::ui::interaction::confirm_inline_yes_no;

pub struct SyncRunOptions {
    pub porcelain: bool,
    pub yes: bool,
    pub dry_run: bool,
    pub debug: bool,
}

pub fn run(
    db: &Database,
    git: &Git,
    provider: &dyn Provider,
    base_branch: &str,
    base_remote: &str,
    opts: SyncRunOptions,
) -> Result<()> {
    let sync_started = Instant::now();
    let plan_started = Instant::now();
    let (plan, plan_timing) = build_sync_plan(db, git, provider, base_branch, base_remote)?;
    let plan_elapsed = plan_started.elapsed();
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
        emit_sync_timing(
            opts.debug,
            plan_elapsed,
            None,
            sync_started.elapsed(),
            &plan_timing,
        );
        return Ok(());
    }

    if plan_view.operations.is_empty() {
        if !opts.porcelain {
            println!("sync already up to date");
        }
        emit_sync_timing(
            opts.debug,
            plan_elapsed,
            None,
            sync_started.elapsed(),
            &plan_timing,
        );
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
        emit_sync_timing(
            opts.debug,
            plan_elapsed,
            None,
            sync_started.elapsed(),
            &plan_timing,
        );
        return Ok(());
    }

    let apply_started = Instant::now();
    crate::core::execute_sync_plan(db, git, provider, &plan)?;
    let apply_elapsed = apply_started.elapsed();
    if !opts.porcelain {
        println!("sync completed");
    }

    if opts.porcelain {
        emit_sync_timing(
            opts.debug,
            plan_elapsed,
            Some(apply_elapsed),
            sync_started.elapsed(),
            &plan_timing,
        );
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
        crate::commands::push::run(
            db,
            git,
            provider,
            &crate::args::PushArgs { push_target: None },
            false,
            opts.yes,
            base_branch,
        )?;
    }

    emit_sync_timing(
        opts.debug,
        plan_elapsed,
        Some(apply_elapsed),
        sync_started.elapsed(),
        &plan_timing,
    );
    Ok(())
}

fn emit_sync_timing(
    debug: bool,
    plan: Duration,
    apply: Option<Duration>,
    total: Duration,
    plan_timing: &SyncPlanTiming,
) {
    if !debug {
        return;
    }

    let apply_ms = apply
        .map(|elapsed| elapsed.as_millis().to_string())
        .unwrap_or_else(|| "n/a".to_string());
    eprintln!(
        "debug: sync timing plan_ms={} apply_ms={} total_ms={} setup_ms={} pr_lookup_ms={} assemble_ms={}",
        plan.as_millis(),
        apply_ms,
        total.as_millis(),
        plan_timing.setup.as_millis(),
        plan_timing.pr_lookup.as_millis(),
        plan_timing.assemble.as_millis()
    );
}
