mod cli;
mod commands;
mod core;
mod db;
mod git;
mod output;
mod provider;
mod tui;
mod util;

use std::collections::HashMap;
use std::io::{IsTerminal, stdin, stdout};

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::style::Stylize;
use output::{BranchView, print_json};
use provider::Provider;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Commands, DeleteArgs, PrArgs, TrackArgs};
use crate::core::render_tree;
use crate::db::{BranchRecord, Database};
use crate::git::Git;
use crate::provider::GithubProvider;

fn main() -> Result<()> {
    if let Err(err) = run() {
        if err
            .downcast_ref::<commands::shared::UserCancelled>()
            .is_some()
        {
            eprintln!("\n{}", "cancelled by user".red().bold());
            std::process::exit(130);
        }
        return Err(err);
    }
    Ok(())
}

fn run() -> Result<()> {
    // Dialoguer Ctrl-C workaround from console-rs/dialoguer#294.
    // We keep SIGINT handler no-op and recover cursor state on prompt errors.
    ctrlc::set_handler(|| {
        // Intentionally no-op: let dialoguer return an interrupted error.
    })
    .context("failed to install Ctrl-C handler")?;

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .compact()
        .init();

    let cli = Cli::parse();
    let git = Git::discover()?;
    let git_dir = git.git_dir()?;
    let db_path = git_dir.join("stack.db");
    let db = Database::open(&db_path)?;
    let default_base = git.default_base_branch()?;
    db.set_base_branch_if_missing(&default_base)?;
    let base_branch = db.repo_meta()?.base_branch;
    let base_remote = git.base_remote_for_stack(&base_branch)?;
    let provider = GithubProvider::new(git.clone(), cli.global.debug);

    match cli.command {
        None => cmd_stack(
            &db,
            &git,
            cli.global.porcelain,
            cli.global.interactive,
            &base_branch,
            &base_remote,
        ),
        Some(Commands::Create(args)) => {
            cmd_create(&db, &git, &args.parent, &args.name, cli.global.porcelain)
        }
        Some(Commands::Track(args)) => cmd_track(
            &db,
            &git,
            &provider,
            &args,
            &base_branch,
            commands::track::TrackRunOptions {
                porcelain: cli.global.porcelain,
                yes: cli.global.yes,
                dry_run: args.dry_run,
                force: args.force,
                debug: cli.global.debug,
            },
        ),
        Some(Commands::Sync(args)) => cmd_sync(
            &db,
            &git,
            &provider,
            &base_branch,
            &base_remote,
            commands::sync::SyncRunOptions {
                porcelain: cli.global.porcelain,
                yes: cli.global.yes,
                dry_run: args.dry_run,
            },
        ),
        Some(Commands::Doctor(args)) => cmd_doctor(&db, &git, cli.global.porcelain, args.fix),
        Some(Commands::Untrack(args)) => cmd_untrack(
            &db,
            &git,
            args.branch.as_deref(),
            cli.global.porcelain,
            &base_branch,
            cli.global.yes,
        ),
        Some(Commands::Delete(args)) => cmd_delete(
            &db,
            &git,
            &provider,
            &args,
            cli.global.porcelain,
            cli.global.yes,
            &base_branch,
        ),
        Some(Commands::Pr(args)) => cmd_pr(
            &db,
            &git,
            &provider,
            &args,
            cli.global.porcelain,
            cli.global.yes,
            cli.global.debug,
        ),
        Some(Commands::Completions(args)) => cmd_completions(args.shell),
    }
}

fn cmd_completions(shell: Option<clap_complete::Shell>) -> Result<()> {
    commands::completions::run(shell)
}

fn cmd_stack(
    db: &Database,
    git: &Git,
    porcelain: bool,
    interactive: bool,
    base_branch: &str,
    base_remote: &str,
) -> Result<()> {
    let records = db.list_branches()?;
    let branch_views = to_branch_views(git, &records)?;

    if porcelain {
        return print_json(&branch_views);
    }

    let is_tty = stdout().is_terminal() && stdin().is_terminal();
    if interactive && is_tty {
        return tui::run_stack_tui(&branch_views);
    }

    let should_color = is_tty && std::env::var_os("NO_COLOR").is_none();
    let pr_base_url = git.remote_web_url(base_remote)?;
    println!(
        "{}",
        render_tree(&records, should_color, pr_base_url.as_deref(), base_branch)
    );
    Ok(())
}

fn cmd_create(
    db: &Database,
    git: &Git,
    parent_arg: &Option<String>,
    name_arg: &Option<String>,
    porcelain: bool,
) -> Result<()> {
    commands::create::run(db, git, parent_arg, name_arg, porcelain)
}

fn cmd_track(
    db: &Database,
    git: &Git,
    provider: &dyn Provider,
    args: &TrackArgs,
    base_branch: &str,
    opts: commands::track::TrackRunOptions,
) -> Result<()> {
    commands::track::run(db, git, provider, args, base_branch, opts)
}

fn cmd_sync(
    db: &Database,
    git: &Git,
    provider: &dyn Provider,
    base_branch: &str,
    base_remote: &str,
    opts: commands::sync::SyncRunOptions,
) -> Result<()> {
    commands::sync::run(db, git, provider, base_branch, base_remote, opts)
}

fn cmd_doctor(db: &Database, git: &Git, porcelain: bool, fix: bool) -> Result<()> {
    commands::doctor::run(db, git, porcelain, fix)
}

fn cmd_untrack(
    db: &Database,
    git: &Git,
    branch_arg: Option<&str>,
    porcelain: bool,
    base_branch: &str,
    yes: bool,
) -> Result<()> {
    commands::untrack::run(db, git, branch_arg, porcelain, base_branch, yes)
}

fn cmd_pr(
    db: &Database,
    git: &Git,
    provider: &dyn Provider,
    args: &PrArgs,
    porcelain: bool,
    yes: bool,
    debug: bool,
) -> Result<()> {
    commands::pr::run(db, git, provider, args, porcelain, yes, debug)
}

fn cmd_delete(
    db: &Database,
    git: &Git,
    provider: &dyn Provider,
    args: &DeleteArgs,
    porcelain: bool,
    yes: bool,
    base_branch: &str,
) -> Result<()> {
    commands::delete::run(db, git, provider, args, porcelain, yes, base_branch)
}

fn to_branch_views(git: &Git, records: &[BranchRecord]) -> Result<Vec<BranchView>> {
    let mut id_map: HashMap<i64, String> = HashMap::new();
    for rec in records {
        id_map.insert(rec.id, rec.name.clone());
    }

    records
        .iter()
        .map(|rec| {
            let exists_in_git = git.branch_exists(&rec.name)?;
            Ok(BranchView {
                name: rec.name.clone(),
                parent: rec.parent_branch_id.and_then(|id| id_map.get(&id).cloned()),
                last_synced_head_sha: rec.last_synced_head_sha.clone(),
                cached_pr_number: rec.cached_pr_number,
                cached_pr_state: rec.cached_pr_state.clone(),
                exists_in_git,
            })
        })
        .collect()
}

#[cfg(test)]
fn prompt_or_cancel<T>(result: dialoguer::Result<T>) -> Result<T> {
    commands::shared::prompt_or_cancel(result)
}

#[cfg(test)]
fn truncate_for_display(value: &str, max_chars: usize) -> String {
    util::terminal::truncate_for_display(value, max_chars)
}

#[cfg(test)]
fn build_branch_picker_items(
    ordered_names: &[String],
    current: &str,
    tracked: &[BranchRecord],
) -> Vec<String> {
    commands::shared::build_branch_picker_items(ordered_names, current, tracked)
}

#[cfg(test)]
fn build_delete_picker_items(
    tracked_names: &[String],
    current: &str,
    tracked: &[BranchRecord],
) -> Vec<String> {
    commands::shared::build_delete_picker_items(tracked_names, current, tracked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_picker_items_include_source_labels() {
        let tracked = vec![BranchRecord {
            id: 1,
            name: "feat/a".to_string(),
            parent_branch_id: None,
            last_synced_head_sha: None,
            cached_pr_number: Some(10),
            cached_pr_state: Some("open".to_string()),
        }];
        let ordered = vec![
            "main".to_string(),
            "feat/a".to_string(),
            "fix/local".to_string(),
        ];
        let items = build_branch_picker_items(&ordered, "main", &tracked);
        assert!(items[0].starts_with("● current"));
        assert!(items[1].starts_with("◆ tracked"));
        assert!(items[2].starts_with("○ local"));
    }

    #[test]
    fn delete_picker_items_highlight_current() {
        let tracked = vec![
            BranchRecord {
                id: 1,
                name: "feat/a".to_string(),
                parent_branch_id: None,
                last_synced_head_sha: None,
                cached_pr_number: Some(10),
                cached_pr_state: Some("open".to_string()),
            },
            BranchRecord {
                id: 2,
                name: "feat/b".to_string(),
                parent_branch_id: None,
                last_synced_head_sha: None,
                cached_pr_number: None,
                cached_pr_state: None,
            },
        ];
        let names = vec!["feat/a".to_string(), "feat/b".to_string()];
        let items = build_delete_picker_items(&names, "feat/b", &tracked);
        assert!(items[0].starts_with("◆ tracked"));
        assert!(items[1].starts_with("● current"));
    }

    #[test]
    fn prompt_interrupt_maps_to_user_cancelled_error() {
        let err = dialoguer::Error::IO(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "ctrl-c",
        ));
        let result = prompt_or_cancel::<()>(Err(err));
        assert!(result.is_err());
        let got = result.unwrap_err();
        assert!(
            got.downcast_ref::<crate::commands::shared::UserCancelled>()
                .is_some()
        );
    }

    #[test]
    fn long_prompt_exceeds_example_width_budget() {
        let prompt = "Open PR from 'main' into 'main' even though the branch is not stacked?";
        let prompt_len = prompt.chars().count();
        let min_options_len = "  ○ Yes   ○ No".chars().count();
        assert!(prompt_len + min_options_len > 74);
    }

    #[test]
    fn truncate_for_display_keeps_short_text() {
        assert_eq!(
            truncate_for_display("https://example.com/pr/1", 40),
            "https://example.com/pr/1"
        );
    }

    #[test]
    fn truncate_for_display_adds_ellipsis_for_long_text() {
        let value =
            "https://github.com/acme/repo/compare/main...very/long/branch/name/with/extra/segments";
        let out = truncate_for_display(value, 32);
        assert!(out.ends_with('…'));
        assert!(out.chars().count() <= 32);
    }
}
