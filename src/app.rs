use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::args::{Cli, Commands};
use crate::commands;
use crate::db::Database;
use crate::git::Git;
use crate::provider::GithubProvider;

pub struct AppContext {
    cli: Cli,
    git: Git,
    db: Database,
    base_branch: String,
    base_remote: String,
    provider: GithubProvider,
}

impl AppContext {
    fn build() -> Result<Self> {
        let cli = Cli::parse();
        let git = Git::discover()?;
        let db_path = prepare_stack_db_path(&git)?;
        let db = Database::open(&db_path)?;
        let default_base = git.default_base_branch()?;
        db.set_base_branch_if_missing(&default_base)?;
        let cached_base = db.repo_meta()?.base_branch;
        if cached_base != default_base && !git.branch_exists(&cached_base)? {
            db.set_base_branch(&default_base)?;
        }
        let base_branch = db.repo_meta()?.base_branch;
        let base_remote = git.base_remote_for_stack(&base_branch)?;
        let provider = GithubProvider::new(git.clone(), cli.global.debug);

        Ok(Self {
            cli,
            git,
            db,
            base_branch,
            base_remote,
            provider,
        })
    }
}

fn prepare_stack_db_path(git: &Git) -> Result<PathBuf> {
    let shared = git.stack_db_path()?;
    let legacy = git.git_dir()?.join("stack.db");
    if legacy == shared || !legacy.exists() {
        return Ok(shared);
    }
    if shared.exists() {
        eprintln!(
            "warning: using shared stack metadata at '{}'; legacy linked-worktree metadata remains at '{}' and must be reconciled manually",
            shared.display(),
            legacy.display()
        );
        return Ok(shared);
    }

    let sidecars = ["-journal", "-wal", "-shm"]
        .map(|suffix| sqlite_sidecar(&legacy, suffix))
        .into_iter()
        .filter(|path| path.exists())
        .collect::<Vec<_>>();
    if !sidecars.is_empty() {
        return Err(anyhow!(
            "cannot migrate legacy stack metadata from '{}' while SQLite sidecar files exist; close other stack processes and recover that database before retrying",
            legacy.display()
        ));
    }

    fs::rename(&legacy, &shared).with_context(|| {
        format!(
            "failed to migrate linked-worktree stack metadata from '{}' to shared path '{}'",
            legacy.display(),
            shared.display()
        )
    })?;
    Ok(shared)
}

fn sqlite_sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut value = OsString::from(path.as_os_str());
    value.push(suffix);
    PathBuf::from(value)
}

pub fn run() -> Result<()> {
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

    let ctx = AppContext::build()?;
    dispatch(&ctx)
}

fn dispatch(ctx: &AppContext) -> Result<()> {
    match &ctx.cli.command {
        None => commands::stack::run(
            &ctx.db,
            &ctx.git,
            ctx.cli.global.porcelain,
            ctx.cli.global.interactive,
            &ctx.base_branch,
            &ctx.base_remote,
        ),
        Some(Commands::Create(args)) => commands::create::run(
            &ctx.db,
            &ctx.git,
            &ctx.provider,
            &args.parent,
            &args.insert,
            &args.name,
            ctx.cli.global.porcelain,
        ),
        Some(Commands::Track(args)) => commands::track::run(
            &ctx.db,
            &ctx.git,
            &ctx.provider,
            args,
            &ctx.base_branch,
            commands::track::TrackRunOptions {
                porcelain: ctx.cli.global.porcelain,
                yes: ctx.cli.global.yes,
                dry_run: args.dry_run,
                force: args.force,
                debug: ctx.cli.global.debug,
            },
        ),
        Some(Commands::Sync(args)) => commands::sync::run(
            &ctx.db,
            &ctx.git,
            &ctx.provider,
            &ctx.base_branch,
            &ctx.base_remote,
            commands::sync::SyncRunOptions {
                porcelain: ctx.cli.global.porcelain,
                yes: ctx.cli.global.yes,
                dry_run: args.dry_run,
                debug: ctx.cli.global.debug,
            },
        ),
        Some(Commands::Doctor(args)) => {
            commands::doctor::run(&ctx.db, &ctx.git, ctx.cli.global.porcelain, args.fix)
        }
        Some(Commands::Untrack(args)) => commands::untrack::run(
            &ctx.db,
            &ctx.git,
            args.branch.as_deref(),
            ctx.cli.global.porcelain,
            &ctx.base_branch,
            ctx.cli.global.yes,
        ),
        Some(Commands::Delete(args)) => commands::delete::run(
            &ctx.db,
            &ctx.git,
            &ctx.provider,
            args,
            ctx.cli.global.porcelain,
            ctx.cli.global.yes,
            &ctx.base_branch,
        ),
        Some(Commands::Rename(args)) => commands::rename::run(
            &ctx.db,
            &ctx.git,
            &ctx.provider,
            args,
            ctx.cli.global.porcelain,
            ctx.cli.global.yes,
            &ctx.base_branch,
        ),
        Some(Commands::Move(args)) => commands::r#move::run(
            &ctx.db,
            &ctx.git,
            &ctx.provider,
            args,
            ctx.cli.global.porcelain,
            &ctx.base_branch,
            &ctx.base_remote,
        ),
        Some(Commands::Split(args)) => commands::split::run(
            &ctx.db,
            &ctx.git,
            args,
            ctx.cli.global.porcelain,
            ctx.cli.global.yes,
            &ctx.base_branch,
        ),
        Some(Commands::Pr(args)) => commands::pr::run(
            &ctx.db,
            &ctx.git,
            &ctx.provider,
            args,
            ctx.cli.global.porcelain,
            ctx.cli.global.yes,
            ctx.cli.global.debug,
        ),
        Some(Commands::Config(args)) => {
            commands::config::run(&ctx.db, &ctx.git, args, ctx.cli.global.porcelain)
        }
        Some(Commands::Push(args)) => commands::push::run(
            &ctx.db,
            &ctx.git,
            &ctx.provider,
            args,
            ctx.cli.global.porcelain,
            ctx.cli.global.yes,
            &ctx.base_branch,
        ),
        Some(Commands::Top) => commands::nav::run(
            &ctx.db,
            &ctx.git,
            commands::nav::NavCommand::Top,
            ctx.cli.global.porcelain,
        ),
        Some(Commands::Bottom) => commands::nav::run(
            &ctx.db,
            &ctx.git,
            commands::nav::NavCommand::Bottom,
            ctx.cli.global.porcelain,
        ),
        Some(Commands::Up) => commands::nav::run(
            &ctx.db,
            &ctx.git,
            commands::nav::NavCommand::Up,
            ctx.cli.global.porcelain,
        ),
        Some(Commands::Down) => commands::nav::run(
            &ctx.db,
            &ctx.git,
            commands::nav::NavCommand::Down,
            ctx.cli.global.porcelain,
        ),
        Some(Commands::Completions(args)) => commands::completions::run(args.shell),
    }
}
