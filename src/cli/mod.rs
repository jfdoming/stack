use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "stack", version, about = "Manage stacked pull requests")]
pub struct Cli {
    #[arg(
        short = 'P',
        long,
        global = true,
        help = "Output machine-readable JSON"
    )]
    pub porcelain: bool,
    #[arg(
        short = 'y',
        long,
        global = true,
        help = "Skip interactive confirmations"
    )]
    pub yes: bool,
    #[arg(
        short = 'i',
        long,
        global = true,
        help = "Launch interactive fullscreen UI for `stack` (no subcommand)"
    )]
    pub interactive: bool,
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Add a branch to the stack
    Create(CreateArgs),
    /// Update stacked branches
    Sync(SyncArgs),
    /// Validate and optionally repair stack metadata
    Doctor(DoctorArgs),
    /// Remove a branch from stack relationships
    Unlink(UnlinkArgs),
    /// Delete a branch and splice it out of the stack
    Delete(DeleteArgs),
    /// Create a pull request for the current branch
    Pr(PrArgs),
    /// Generate shell completion scripts
    Completions(CompletionsArgs),
}

#[derive(Debug, Args)]
pub struct CreateArgs {
    #[arg(short = 'p', long, help = "Parent branch name")]
    pub parent: Option<String>,
    #[arg(short = 'n', long, help = "Child branch name to create")]
    pub name: Option<String>,
}

#[derive(Debug, Args)]
pub struct SyncArgs {
    #[arg(short = 'n', long, help = "Plan only; do not execute git operations")]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct DoctorArgs {
    #[arg(short = 'f', long, help = "Apply maintenance fixes")]
    pub fix: bool,
}

#[derive(Debug, Args)]
pub struct UnlinkArgs {
    #[arg(help = "Branch to unlink")]
    pub branch: String,
    #[arg(short = 'd', long, help = "Remove branch record entirely")]
    pub drop_record: bool,
}

#[derive(Debug, Args)]
pub struct DeleteArgs {
    #[arg(help = "Branch to delete (defaults to current branch)")]
    pub branch: Option<String>,
    #[arg(short = 'n', long, help = "Preview delete without mutating git or DB")]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct PrArgs {
    #[arg(short = 't', long, help = "PR title")]
    pub title: Option<String>,
    #[arg(short = 'b', long, help = "PR body")]
    pub body: Option<String>,
    #[arg(short = 'd', long, help = "Create draft PR")]
    pub draft: bool,
    #[arg(short = 'n', long, help = "Preview command without calling gh")]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct CompletionsArgs {
    #[arg(help = "Shell to generate completions for")]
    pub shell: clap_complete::Shell,
}
