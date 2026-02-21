use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "stack", version, about = "Manage stacked pull requests")]
pub struct Cli {
    #[command(flatten, next_help_heading = "Global Options")]
    pub global: GlobalArgs,
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Args)]
pub struct GlobalArgs {
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
    #[arg(long, global = true, help = "Print detailed provider/debug errors")]
    pub debug: bool,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Add a branch to the stack
    Create(CreateArgs),
    /// Track existing branch relationships
    Track(TrackArgs),
    /// Update stacked branches
    Sync(SyncArgs),
    /// Validate and optionally repair stack metadata
    Doctor(DoctorArgs),
    /// Fully untrack a branch from stack relationships
    Untrack(UntrackArgs),
    /// Delete a branch and splice it out of the stack
    Delete(DeleteArgs),
    /// Create a pull request for the current branch
    Pr(PrArgs),
    /// Push tracked branches with force-with-lease
    Push,
    /// Switch to the highest descendant in the current stack path
    Top,
    /// Switch to the stack root ancestor for the current branch
    Bottom,
    /// Switch to a direct child branch
    Up,
    /// Switch to the direct parent branch
    Down,
    /// Generate shell completion scripts
    Completions(CompletionsArgs),
}

#[derive(Debug, Args)]
pub struct CreateArgs {
    #[arg(short = 'p', long, help = "Parent branch name")]
    pub parent: Option<String>,
    #[arg(
        long,
        value_name = "CHILD",
        num_args = 0..=1,
        default_missing_value = "",
        help = "Insert new branch before a tracked child branch",
        conflicts_with = "parent"
    )]
    pub insert: Option<String>,
    #[arg(short = 'n', long, help = "Child branch name to create")]
    pub name: Option<String>,
}

#[derive(Debug, Args)]
pub struct TrackArgs {
    #[arg(help = "Existing branch to track")]
    pub branch: Option<String>,
    #[arg(long, help = "Track all local non-base branches")]
    pub all: bool,
    #[arg(short = 'p', long, help = "Parent branch name")]
    pub parent: Option<String>,
    #[arg(
        long,
        help = "Infer parent only (skip interactive parent selection fallback)"
    )]
    pub infer: bool,
    #[arg(short = 'n', long, help = "Preview changes without mutating DB")]
    pub dry_run: bool,
    #[arg(
        short = 'f',
        long,
        help = "Replace existing parent links in non-interactive mode"
    )]
    pub force: bool,
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
pub struct UntrackArgs {
    #[arg(help = "Branch to untrack (defaults to interactive selection)")]
    pub branch: Option<String>,
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
#[command(
    after_help = "Installation examples:\n  zsh:        stack completions zsh > ~/.zsh/completions/_stack\n  bash:       stack completions bash > ~/.local/share/bash-completion/completions/stack\n  fish:       stack completions fish > ~/.config/fish/completions/stack.fish\n  powershell: stack completions powershell > stack.ps1"
)]
pub struct CompletionsArgs {
    #[arg(help = "Shell to generate completions for")]
    pub shell: Option<clap_complete::Shell>,
}
