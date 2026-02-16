mod cli;
mod commands;
mod core;
mod db;
mod git;
mod output;
mod provider;
mod tui;
mod util;

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::style::Stylize;
use provider::Provider;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Commands, DeleteArgs, PrArgs, TrackArgs};
use crate::db::Database;
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
    commands::stack::run(db, git, porcelain, interactive, base_branch, base_remote)
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
