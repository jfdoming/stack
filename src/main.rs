mod args;
mod commands;
mod core;
mod db;
mod git;
mod views;
mod provider;

mod ui;
mod util;

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::style::Stylize;
use tracing_subscriber::EnvFilter;

use crate::args::{Cli, Commands};
use crate::db::Database;
use crate::git::Git;
use crate::provider::GithubProvider;

fn main() -> Result<()> {
    if let Err(err) = run() {
        if err
            .downcast_ref::<ui::interaction::UserCancelled>()
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
        None => commands::stack::run(
            &db,
            &git,
            cli.global.porcelain,
            cli.global.interactive,
            &base_branch,
            &base_remote,
        ),
        Some(Commands::Create(args)) => {
            commands::create::run(&db, &git, &args.parent, &args.name, cli.global.porcelain)
        }
        Some(Commands::Track(args)) => commands::track::run(
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
        Some(Commands::Sync(args)) => commands::sync::run(
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
        Some(Commands::Doctor(args)) => {
            commands::doctor::run(&db, &git, cli.global.porcelain, args.fix)
        }
        Some(Commands::Untrack(args)) => commands::untrack::run(
            &db,
            &git,
            args.branch.as_deref(),
            cli.global.porcelain,
            &base_branch,
            cli.global.yes,
        ),
        Some(Commands::Delete(args)) => commands::delete::run(
            &db,
            &git,
            &provider,
            &args,
            cli.global.porcelain,
            cli.global.yes,
            &base_branch,
        ),
        Some(Commands::Pr(args)) => commands::pr::run(
            &db,
            &git,
            &provider,
            &args,
            cli.global.porcelain,
            cli.global.yes,
            cli.global.debug,
        ),
        Some(Commands::Completions(args)) => commands::completions::run(args.shell),
    }
}
