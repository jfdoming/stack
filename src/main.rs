mod cli;
mod core;
mod db;
mod git;
mod output;
mod provider;
mod tui;

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::io::{IsTerminal, stdin, stdout};

use anyhow::{Context, Result, anyhow};
use clap::{CommandFactory, Parser};
use crossterm::cursor::{Hide, MoveToColumn, RestorePosition, SavePosition, Show};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::style::Stylize;
use crossterm::terminal::{Clear, ClearType, disable_raw_mode, enable_raw_mode};
use dialoguer::console::Term;
use dialoguer::{Input, Select, theme::ColorfulTheme};
use output::{BranchView, DoctorIssueView, print_json};
use provider::{CreatePrRequest, Provider};
use thiserror::Error;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Commands, DeleteArgs, PrArgs, TrackArgs};
use crate::core::{build_sync_plan, rank_parent_candidates, render_tree};
use crate::db::{BranchRecord, Database, ParentUpdate};
use crate::git::Git;
use crate::provider::GithubProvider;

#[derive(Debug, Error)]
#[error("cancelled by user")]
struct UserCancelled;

struct SyncRunOptions {
    porcelain: bool,
    yes: bool,
    dry_run: bool,
}

#[derive(Debug, Clone)]
struct TrackRunOptions {
    porcelain: bool,
    yes: bool,
    dry_run: bool,
    force: bool,
    debug: bool,
}

#[derive(Debug, Clone, Copy)]
enum TrackSource {
    Explicit,
    PrBase,
    GitAncestry,
}

impl TrackSource {
    fn as_str(self) -> &'static str {
        match self {
            TrackSource::Explicit => "explicit",
            TrackSource::PrBase => "pr_base",
            TrackSource::GitAncestry => "git_ancestry",
        }
    }
}

#[derive(Debug, Clone)]
struct ParentInference {
    parent: String,
    source: TrackSource,
    confidence: &'static str,
}

#[derive(Debug, Clone)]
struct TrackChange {
    branch: String,
    old_parent: Option<String>,
    new_parent: String,
    source: TrackSource,
    confidence: &'static str,
}

#[derive(Debug, Clone)]
struct TrackSkip {
    branch: String,
    reason: String,
}

fn main() -> Result<()> {
    if let Err(err) = run() {
        if err.downcast_ref::<UserCancelled>().is_some() {
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
            TrackRunOptions {
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
            SyncRunOptions {
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
    let shell = if let Some(shell) = shell {
        shell
    } else if stdout().is_terminal() && stdin().is_terminal() {
        let theme = ColorfulTheme::default();
        let shells = [
            clap_complete::Shell::Bash,
            clap_complete::Shell::Zsh,
            clap_complete::Shell::Fish,
            clap_complete::Shell::Elvish,
            clap_complete::Shell::PowerShell,
        ];
        let labels = ["bash", "zsh", "fish", "elvish", "powershell"];
        let idx = prompt_or_cancel(
            Select::with_theme(&theme)
                .with_prompt("Select shell for completion script")
                .items(&labels)
                .default(1)
                .interact(),
        )?;
        shells[idx]
    } else {
        return Err(anyhow!(
            "shell required in non-interactive mode; pass stack completions <shell>"
        ));
    };

    let mut cmd = Cli::command();
    let bin_name = cmd.get_name().to_string();
    clap_complete::generate(shell, &mut cmd, bin_name, &mut std::io::stdout());
    Ok(())
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
    let current = git.current_branch()?;
    let tracked = db.list_branches()?;
    let local = git.local_branches()?;
    let parent_candidates = rank_parent_candidates(&current, &tracked, &local);
    let picker_items = build_branch_picker_items(&parent_candidates, &current, &tracked);
    let theme = ColorfulTheme::default();

    let parent = if let Some(parent) = parent_arg {
        parent.clone()
    } else if parent_candidates.len() == 1 {
        let assumed = parent_candidates[0].clone();
        if !porcelain {
            println!("assuming parent branch '{assumed}' (only viable branch)");
        }
        assumed
    } else if stdout().is_terminal() && stdin().is_terminal() {
        let default_idx = parent_candidates
            .iter()
            .position(|b| b == &current)
            .unwrap_or(0);
        let idx = prompt_or_cancel(
            Select::with_theme(&theme)
                .with_prompt(
                    "Select parent branch (↑/↓ to navigate, Enter to select, Ctrl-C to cancel)",
                )
                .items(&picker_items)
                .default(default_idx)
                .interact(),
        )?;
        parent_candidates[idx].clone()
    } else {
        return Err(anyhow!(
            "parent required in non-interactive mode; pass --parent <branch>"
        ));
    };

    if !git.branch_exists(&parent)? {
        return Err(anyhow!("parent branch does not exist in git: {parent}"));
    }

    let child = if let Some(name) = name_arg {
        name.clone()
    } else if stdout().is_terminal() && stdin().is_terminal() {
        prompt_or_cancel(
            Input::<String>::with_theme(&theme)
                .with_prompt("Name for new child branch")
                .validate_with(|input: &String| -> Result<(), &str> {
                    if input.trim().is_empty() {
                        Err("branch name cannot be empty")
                    } else {
                        Ok(())
                    }
                })
                .interact_text(),
        )?
    } else {
        return Err(anyhow!(
            "branch name required in non-interactive mode; pass --name <branch>"
        ));
    };

    if git.branch_exists(&child)? {
        return Err(anyhow!("branch already exists: {child}"));
    }

    git.create_branch_from(&child, &parent)
        .with_context(|| format!("failed to create branch {child} from {parent}"))?;

    db.set_parent(&child, Some(&parent))?;
    let child_sha = git.head_sha(&child)?;
    let create_url = if let Some(remote) = git.remote_for_branch(&parent)? {
        if let Some(base) = git.remote_web_url(&remote)? {
            format!(
                "{}/compare/{}...{}?expand=1",
                base.trim_end_matches('/'),
                parent,
                child
            )
        } else {
            String::new()
        }
    } else {
        String::new()
    };
    db.set_sync_sha(&child, &child_sha)?;
    let out = serde_json::json!({
        "created": child,
        "parent": parent,
        "head_sha": child_sha,
        "db": db_summary_path(git)?,
        "create_url": create_url,
    });

    if porcelain {
        print_json(&out)?;
    } else {
        let use_color = stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
        if use_color {
            println!(
                "created stack branch: {} -> {}{}",
                out["parent"].as_str().unwrap_or("<unknown>").green().bold(),
                out["created"].as_str().unwrap_or("<unknown>").cyan().bold(),
                if out["create_url"].as_str().unwrap_or_default().is_empty() {
                    String::new()
                } else {
                    format!(
                        " {}",
                        osc8_hyperlink(
                            out["create_url"].as_str().unwrap_or_default(),
                            "open compare",
                        )
                        .dark_grey()
                        .underlined()
                    )
                }
            );
        } else {
            println!(
                "created stack branch: {} -> {}{}",
                out["parent"],
                out["created"],
                if out["create_url"].as_str().unwrap_or_default().is_empty() {
                    String::new()
                } else {
                    format!(" {}", out["create_url"])
                }
            );
        }
    }
    Ok(())
}

fn cmd_track(
    db: &Database,
    git: &Git,
    provider: &dyn Provider,
    args: &TrackArgs,
    base_branch: &str,
    opts: TrackRunOptions,
) -> Result<()> {
    if args.all && args.branch.is_some() {
        return Err(anyhow!(
            "cannot combine --all with a positional branch argument"
        ));
    }
    if args.all && args.parent.is_some() {
        return Err(anyhow!("cannot combine --all with --parent"));
    }

    let is_tty = stdout().is_terminal() && stdin().is_terminal();
    let current = git.current_branch()?;
    let tracked = db.list_branches()?;
    let by_name: HashMap<String, BranchRecord> = tracked
        .iter()
        .map(|b| (b.name.clone(), b.clone()))
        .collect();
    let by_id: HashMap<i64, String> = tracked.iter().map(|b| (b.id, b.name.clone())).collect();
    let local = git.local_branches()?;
    let local_set: HashSet<String> = local.iter().cloned().collect();
    let mut changes = Vec::new();
    let mut skipped = Vec::new();
    let mut unresolved = Vec::new();
    let mut warnings = Vec::new();

    let mut assumed_target: Option<String> = None;
    let targets: Vec<String> = if args.all {
        local
            .iter()
            .filter(|b| b.as_str() != base_branch)
            .cloned()
            .collect()
    } else if let Some(branch) = &args.branch {
        vec![branch.clone()]
    } else {
        let viable_names: Vec<String> = local
            .iter()
            .filter(|b| b.as_str() != base_branch)
            .cloned()
            .collect();
        if viable_names.is_empty() {
            return Err(anyhow!("no local non-base branches available to track"));
        }
        if viable_names.len() == 1 {
            let assumed = viable_names[0].clone();
            if !opts.porcelain {
                println!("assuming target branch '{assumed}' (only viable branch)");
            }
            assumed_target = Some(assumed.clone());
            vec![assumed]
        } else if is_tty {
            let theme = ColorfulTheme::default();
            let picker_items = build_branch_picker_items(&viable_names, &current, &tracked);
            let default_idx = viable_names.iter().position(|b| b == &current).unwrap_or(0);
            let idx = prompt_or_cancel(
                Select::with_theme(&theme)
                    .with_prompt(
                        "Select branch to track (↑/↓ to navigate, Enter to select, Ctrl-C to cancel)",
                    )
                    .items(&picker_items)
                    .default(default_idx)
                    .interact(),
            )?;
            vec![viable_names[idx].clone()]
        } else {
            return Err(anyhow!(
                "branch required in non-interactive mode; pass stack track <branch>"
            ));
        }
    };

    if let Some(assumed) = &assumed_target
        && !opts.yes
        && !opts.dry_run
    {
        if is_tty {
            let confirmed =
                confirm_inline_yes_no(&format!("Track assumed target branch '{assumed}'?"))?;
            if !confirmed {
                if !opts.porcelain {
                    println!("track not applied: confirmation declined; no changes made");
                }
                return Ok(());
            }
        } else {
            return Err(anyhow!(
                "target branch was auto-selected as '{}'; rerun with an explicit branch or pass --yes",
                assumed
            ));
        }
    }

    for target in targets {
        if !local_set.contains(&target) {
            return Err(anyhow!("branch '{}' does not exist in git", target));
        }
        if target == base_branch {
            skipped.push(TrackSkip {
                branch: target,
                reason: "base branch is not eligible for tracking".to_string(),
            });
            continue;
        }

        let inference = if args.all {
            infer_parent_for_branch(
                git,
                provider,
                &target,
                by_name.get(&target),
                &local,
                &mut warnings,
                opts.debug,
            )?
        } else if let Some(parent) = &args.parent {
            if !local_set.contains(parent) {
                return Err(anyhow!("parent branch does not exist in git: {}", parent));
            }
            Some(ParentInference {
                parent: parent.clone(),
                source: TrackSource::Explicit,
                confidence: "high",
            })
        } else {
            let inferred = infer_parent_for_branch(
                git,
                provider,
                &target,
                by_name.get(&target),
                &local,
                &mut warnings,
                opts.debug,
            )?;
            if inferred.is_some() || args.infer {
                inferred
            } else {
                let parent_candidates: Vec<String> =
                    rank_parent_candidates(&target, &tracked, &local)
                        .into_iter()
                        .filter(|candidate| candidate != &target)
                        .collect();
                if parent_candidates.is_empty() {
                    return Err(anyhow!(
                        "no viable parent branches available for '{}'",
                        target
                    ));
                }
                let parent = if parent_candidates.len() == 1 {
                    let assumed = parent_candidates[0].clone();
                    if !opts.porcelain {
                        println!("assuming parent branch '{assumed}' (only viable branch)");
                    }
                    assumed
                } else if is_tty {
                    let theme = ColorfulTheme::default();
                    let picker_items =
                        build_branch_picker_items(&parent_candidates, &current, &tracked);
                    let default_idx = parent_candidates
                        .iter()
                        .position(|b| b == &current)
                        .unwrap_or(0);
                    let idx = prompt_or_cancel(
                        Select::with_theme(&theme)
                            .with_prompt(format!(
                                "Select parent branch for '{}' (↑/↓ to navigate, Enter to select, Ctrl-C to cancel)",
                                target
                            ))
                            .items(&picker_items)
                            .default(default_idx)
                            .interact(),
                    )?;
                    parent_candidates[idx].clone()
                } else {
                    return Err(anyhow!(
                        "could not infer a parent in non-interactive mode; pass --parent <branch> or use --infer to allow unresolved output"
                    ));
                };
                Some(ParentInference {
                    parent,
                    source: TrackSource::Explicit,
                    confidence: "high",
                })
            }
        };

        let Some(parent) = inference else {
            unresolved.push(target);
            continue;
        };

        if parent.parent == target {
            unresolved.push(target);
            continue;
        }
        if !local_set.contains(&parent.parent) {
            return Err(anyhow!(
                "inferred parent branch does not exist in git: {}",
                parent.parent
            ));
        }

        let old_parent = by_name
            .get(&target)
            .and_then(|rec| rec.parent_branch_id)
            .and_then(|id| by_id.get(&id).cloned());
        if old_parent.as_deref() == Some(parent.parent.as_str()) {
            skipped.push(TrackSkip {
                branch: target,
                reason: "already linked to inferred parent".to_string(),
            });
            continue;
        }

        changes.push(TrackChange {
            branch: target,
            old_parent,
            new_parent: parent.parent,
            source: parent.source,
            confidence: parent.confidence,
        });
    }

    let mut apply_changes = Vec::new();
    for change in changes {
        if change.old_parent.is_some() && change.old_parent.as_deref() != Some(&change.new_parent) {
            if opts.yes {
                apply_changes.push(change);
                continue;
            }
            if !is_tty {
                if !opts.force {
                    return Err(anyhow!(
                        "parent conflict for '{}': existing '{}' and proposed '{}' (use --force in non-interactive mode)",
                        change.branch,
                        change.old_parent.as_deref().unwrap_or("<none>"),
                        change.new_parent
                    ));
                }
                apply_changes.push(change);
                continue;
            }

            match prompt_track_conflict(&change)? {
                TrackConflictResolution::Replace => apply_changes.push(change),
                TrackConflictResolution::Skip => skipped.push(TrackSkip {
                    branch: change.branch,
                    reason: "conflict skipped by user".to_string(),
                }),
                TrackConflictResolution::Abort => return Err(UserCancelled.into()),
            }
        } else {
            apply_changes.push(change);
        }
    }

    let applied = !opts.dry_run && !apply_changes.is_empty();
    if applied {
        let updates: Vec<ParentUpdate> = apply_changes
            .iter()
            .map(|c| ParentUpdate {
                child_name: c.branch.clone(),
                parent_name: Some(c.new_parent.clone()),
            })
            .collect();
        db.set_parents_batch(&updates)?;
    }

    let changes_payload: Vec<serde_json::Value> = apply_changes
        .iter()
        .map(|c| {
            serde_json::json!({
                "branch": c.branch,
                "old_parent": c.old_parent,
                "new_parent": c.new_parent,
                "source": c.source.as_str(),
                "confidence": c.confidence,
            })
        })
        .collect();
    let skipped_payload: Vec<serde_json::Value> = skipped
        .iter()
        .map(|s| serde_json::json!({"branch": s.branch, "reason": s.reason}))
        .collect();

    let payload = serde_json::json!({
        "mode": if args.all { "all" } else { "single" },
        "dry_run": opts.dry_run,
        "applied": applied,
        "changes": changes_payload,
        "skipped": skipped_payload,
        "unresolved": unresolved,
        "warnings": warnings,
    });

    if opts.porcelain {
        print_json(&payload)?;
        if args.all && !opts.dry_run && !is_tty && !unresolved.is_empty() {
            return Err(anyhow!("some branches could not be resolved"));
        }
        return Ok(());
    }

    for change in apply_changes.iter() {
        println!(
            "{} '{}' under '{}' (source: {}, confidence: {})",
            if opts.dry_run {
                "would track"
            } else {
                "tracking"
            },
            change.branch,
            change.new_parent,
            change.source.as_str(),
            change.confidence
        );
    }
    for skip in skipped {
        println!("skipped '{}': {}", skip.branch, skip.reason);
    }
    for branch in &unresolved {
        println!("could not determine a parent for '{}'", branch);
    }
    for warning in &warnings {
        eprintln!("warning: {warning}");
    }

    if opts.dry_run {
        println!("track dry run complete; no changes were made");
    } else if applied {
        println!("tracking updated");
    } else {
        println!("no tracking changes were needed");
    }

    if args.all && !opts.dry_run && !is_tty && !unresolved.is_empty() {
        return Err(anyhow!("some branches could not be resolved"));
    }
    Ok(())
}

fn infer_parent_for_branch(
    git: &Git,
    provider: &dyn Provider,
    branch: &str,
    tracked: Option<&BranchRecord>,
    local: &[String],
    warnings: &mut Vec<String>,
    debug: bool,
) -> Result<Option<ParentInference>> {
    let cached_number = tracked.and_then(|r| r.cached_pr_number);
    match provider.resolve_pr_by_head(branch, cached_number) {
        Ok(Some(pr)) => {
            if let Some(base) = pr.base_ref_name
                && base != branch
                && git.branch_exists(&base)?
            {
                return Ok(Some(ParentInference {
                    parent: base,
                    source: TrackSource::PrBase,
                    confidence: "high",
                }));
            }
        }
        Ok(None) => {}
        Err(err) => warnings.push(format_pr_metadata_warning(branch, &err, debug)),
    }

    infer_parent_from_git(git, branch, local)
}

fn format_pr_metadata_warning(branch: &str, err: &anyhow::Error, debug: bool) -> String {
    let raw = err.to_string();
    if debug {
        return format!(
            "could not read PR metadata for '{}'; falling back to git ancestry ({})",
            branch, raw
        );
    }
    if raw.contains("expected value at line 1 column 1")
        || raw.contains("EOF while parsing")
        || raw.contains("trailing characters")
    {
        return format!(
            "could not read PR metadata for '{}'; gh returned an unexpected response. Falling back to git ancestry.",
            branch
        );
    }
    format!(
        "could not read PR metadata for '{}'; falling back to git ancestry ({})",
        branch, raw
    )
}

fn infer_parent_from_git(
    git: &Git,
    branch: &str,
    local: &[String],
) -> Result<Option<ParentInference>> {
    let mut best_parent: Option<String> = None;
    let mut best_distance = u32::MAX;
    let mut tied = false;
    for candidate in local {
        if candidate == branch {
            continue;
        }
        if !git.is_ancestor(candidate, branch)? {
            continue;
        }
        let distance = git.commit_distance(candidate, branch)?;
        if distance < best_distance {
            best_parent = Some(candidate.clone());
            best_distance = distance;
            tied = false;
        } else if distance == best_distance {
            tied = true;
        }
    }

    if tied {
        return Ok(None);
    }
    Ok(best_parent.map(|parent| ParentInference {
        parent,
        source: TrackSource::GitAncestry,
        confidence: "medium",
    }))
}

enum TrackConflictResolution {
    Replace,
    Skip,
    Abort,
}

fn prompt_track_conflict(change: &TrackChange) -> Result<TrackConflictResolution> {
    let theme = ColorfulTheme::default();
    let items = vec![
        "Replace parent".to_string(),
        "Skip branch".to_string(),
        "Abort".to_string(),
    ];
    let old = change.old_parent.as_deref().unwrap_or("<none>");
    let idx = prompt_or_cancel(
        Select::with_theme(&theme)
            .with_prompt(format!(
                "Parent conflict for '{}' (existing: '{}', proposed: '{}')",
                change.branch, old, change.new_parent
            ))
            .items(&items)
            .default(0)
            .interact(),
    )?;
    Ok(match idx {
        0 => TrackConflictResolution::Replace,
        1 => TrackConflictResolution::Skip,
        _ => TrackConflictResolution::Abort,
    })
}

fn cmd_sync(
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
        print_json(&plan_view)?;
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

    core::execute_sync_plan(db, git, &plan)?;
    if !opts.porcelain {
        println!("sync completed");
    }
    Ok(())
}

fn cmd_doctor(db: &Database, git: &Git, porcelain: bool, fix: bool) -> Result<()> {
    let records = db.list_branches()?;
    let mut issues = Vec::new();
    let mut id_to_name = HashMap::new();

    for branch in &records {
        id_to_name.insert(branch.id, branch.name.clone());
        if !git.branch_exists(&branch.name)? {
            issues.push(DoctorIssueView {
                severity: "error".to_string(),
                code: "missing_git_branch".to_string(),
                message: format!("tracked branch '{}' does not exist in git", branch.name),
                branch: Some(branch.name.clone()),
            });
            if fix {
                db.delete_branch(&branch.name)?;
            }
        }
    }

    for branch in &records {
        if let Some(pid) = branch.parent_branch_id
            && !id_to_name.contains_key(&pid)
        {
            issues.push(DoctorIssueView {
                severity: "error".to_string(),
                code: "missing_parent_record".to_string(),
                message: format!(
                    "branch '{}' points to unknown parent id {}",
                    branch.name, pid
                ),
                branch: Some(branch.name.clone()),
            });
            if fix {
                db.clear_parent(&branch.name)?;
            }
        }
    }

    issues.extend(cycle_issues(&records));

    if porcelain {
        return print_json(&serde_json::json!({ "issues": issues, "fix_applied": fix }));
    }

    if issues.is_empty() {
        println!("doctor: no issues found");
    } else {
        println!("doctor: {} issue(s)", issues.len());
        for issue in &issues {
            println!("- [{}] {}: {}", issue.severity, issue.code, issue.message);
        }
    }
    if fix {
        println!("doctor maintenance applied where possible");
    }

    Ok(())
}

fn cmd_untrack(
    db: &Database,
    git: &Git,
    branch_arg: Option<&str>,
    porcelain: bool,
    base_branch: &str,
    yes: bool,
) -> Result<()> {
    let current = git.current_branch()?;
    let records = db.list_branches()?;
    let viable_names: Vec<String> = records
        .iter()
        .filter(|r| r.name != base_branch)
        .map(|r| r.name.clone())
        .collect();

    let mut assumed_target = false;
    let branch = if let Some(branch) = branch_arg {
        branch.to_string()
    } else if viable_names.is_empty() {
        return Err(anyhow!("no tracked non-base branches available to untrack"));
    } else if viable_names.len() == 1 {
        let assumed = viable_names[0].clone();
        if !porcelain {
            println!("assuming target branch '{assumed}' (only viable branch)");
        }
        assumed_target = true;
        assumed
    } else if stdout().is_terminal() && stdin().is_terminal() {
        let theme = ColorfulTheme::default();
        let picker_items = build_delete_picker_items(&viable_names, &current, &records);
        let default_idx = viable_names.iter().position(|b| b == &current).unwrap_or(0);
        let idx = prompt_or_cancel(
            Select::with_theme(&theme)
                .with_prompt(
                    "Select branch to untrack (↑/↓ to navigate, Enter to select, Ctrl-C to cancel)",
                )
                .items(&picker_items)
                .default(default_idx)
                .interact(),
        )?;
        viable_names[idx].clone()
    } else {
        return Err(anyhow!(
            "branch required in non-interactive mode; pass stack untrack <branch>"
        ));
    };

    if assumed_target && !yes {
        if stdout().is_terminal() && stdin().is_terminal() {
            let confirmed =
                confirm_inline_yes_no(&format!("Untrack assumed target branch '{branch}'?"))?;
            if !confirmed {
                if !porcelain {
                    println!("untrack not applied: confirmation declined; no changes made");
                }
                return Ok(());
            }
        } else {
            return Err(anyhow!(
                "target branch was auto-selected as '{}'; rerun with an explicit branch or pass --yes",
                branch
            ));
        }
    }

    if db.branch_by_name(&branch)?.is_none() {
        return Err(anyhow!("branch '{}' is not tracked", branch));
    }

    db.splice_out_branch(&branch)?;

    let payload = serde_json::json!({
        "branch": branch,
        "action": "untrack",
        "status": "ok"
    });

    if porcelain {
        print_json(&payload)?;
    } else {
        println!("removed '{branch}' from the stack and re-linked its child branches");
    }

    Ok(())
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
    let current = git.current_branch()?;
    let records = db.list_branches()?;
    let by_id: HashMap<i64, &BranchRecord> = records.iter().map(|r| (r.id, r)).collect();
    let default_base = db.repo_meta()?.base_branch;
    let current_record = records.iter().find(|r| r.name == current);
    let (base, cached_number, non_stacked_reason): (String, Option<i64>, Option<String>) =
        match current_record {
            Some(record) => match record
                .parent_branch_id
                .and_then(|parent_id| by_id.get(&parent_id).map(|r| r.name.clone()))
            {
                Some(parent) => (parent, record.cached_pr_number, None),
                None => (
                    default_base,
                    record.cached_pr_number,
                    Some("branch is tracked but has no parent link".to_string()),
                ),
            },
            None => (
                default_base,
                None,
                Some("branch is not tracked in the stack".to_string()),
            ),
        };

    if let Some(reason) = &non_stacked_reason {
        eprintln!(
            "warning: '{}' is not stacked ({}); using base branch '{}' for PR",
            current, reason, base
        );
    }

    let existing = match provider.resolve_pr_by_head(&current, cached_number) {
        Ok(existing) => existing,
        Err(err) => {
            if debug {
                eprintln!(
                    "warning: could not determine existing PR status from gh; continuing without duplicate check ({})",
                    err
                );
            } else {
                eprintln!(
                    "warning: could not determine existing PR status from gh; continuing without duplicate check"
                );
            }
            None
        }
    };

    let payload = serde_json::json!({
        "head": current,
        "base": base,
        "title": args.title,
        "draft": args.draft,
        "dry_run": args.dry_run,
        "existing_pr_number": existing.as_ref().map(|pr| pr.number),
        "will_create": existing.is_none(),
    });

    if args.dry_run {
        if porcelain {
            return print_json(&payload);
        }
        if let Some(number) = payload["existing_pr_number"].as_i64() {
            println!(
                "PR already exists for '{}': #{}",
                payload["head"].as_str().unwrap_or_default(),
                number
            );
        } else {
            println!(
                "would create PR with base={} head={}",
                payload["base"], payload["head"]
            );
        }
        return Ok(());
    }

    if let Some(number) = payload["existing_pr_number"].as_i64() {
        if porcelain {
            return print_json(&payload);
        }
        println!(
            "PR already exists for '{}': #{}",
            payload["head"].as_str().unwrap_or_default(),
            number
        );
        return Ok(());
    }

    let should_create = if yes {
        true
    } else if stdout().is_terminal() && stdin().is_terminal() {
        let prompt = if non_stacked_reason.is_some() {
            format!(
                "Open PR from '{}' into '{}' even though the branch is not stacked?",
                payload["head"].as_str().unwrap_or_default(),
                payload["base"].as_str().unwrap_or_default()
            )
        } else {
            format!(
                "Open PR from '{}' into '{}'?",
                payload["head"].as_str().unwrap_or_default(),
                payload["base"].as_str().unwrap_or_default()
            )
        };
        confirm_inline_yes_no(&prompt)?
    } else {
        return Err(anyhow!(
            "confirmation required in non-interactive mode; rerun with --yes"
        ));
    };

    if !should_create {
        if !porcelain {
            println!("PR not created: confirmation declined; no changes made");
        }
        return Ok(());
    }

    let result = provider.create_pr(CreatePrRequest {
        head: payload["head"].as_str().unwrap_or_default(),
        base: payload["base"].as_str().unwrap_or_default(),
        title: args.title.as_deref(),
        body: args.body.as_deref(),
        draft: args.draft,
    })?;

    if porcelain {
        return print_json(&serde_json::json!({
            "head": payload["head"],
            "base": payload["base"],
            "url": result.url,
        }));
    }

    if result.url.is_empty() {
        println!("PR creation command executed, but no URL was returned by gh");
    } else {
        println!("created PR: {}", result.url);
    }
    Ok(())
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
            return print_json(&payload);
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
        return print_json(&serde_json::json!({
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

fn db_summary_path(git: &Git) -> Result<String> {
    Ok(git.git_dir()?.join("stack.db").display().to_string())
}

fn cycle_issues(records: &[BranchRecord]) -> Vec<DoctorIssueView> {
    let mut issues = Vec::new();
    let mut by_id: HashMap<i64, &BranchRecord> = HashMap::new();
    for r in records {
        by_id.insert(r.id, r);
    }

    for r in records {
        let mut seen = HashSet::new();
        let mut cursor = r.parent_branch_id;
        while let Some(id) = cursor {
            if !seen.insert(id) {
                issues.push(DoctorIssueView {
                    severity: "error".to_string(),
                    code: "cycle".to_string(),
                    message: format!("cycle detected starting at '{}'", r.name),
                    branch: Some(r.name.clone()),
                });
                break;
            }
            cursor = by_id.get(&id).and_then(|p| p.parent_branch_id);
        }
    }

    issues
}

fn prompt_or_cancel<T>(result: dialoguer::Result<T>) -> Result<T> {
    match result {
        Ok(value) => Ok(value),
        Err(err) => {
            let _ = Term::stdout().show_cursor();
            let _ = Term::stderr().show_cursor();
            match err {
                dialoguer::Error::IO(io_err)
                    if io_err.kind() == std::io::ErrorKind::Interrupted =>
                {
                    Err(UserCancelled.into())
                }
                other => Err(other.into()),
            }
        }
    }
}

fn osc8_hyperlink(url: &str, label: &str) -> String {
    format!("\u{1b}]8;;{url}\u{1b}\\{label}\u{1b}]8;;\u{1b}\\")
}

fn confirm_inline_yes_no(prompt: &str) -> Result<bool> {
    let mut out = std::io::stdout();
    enable_raw_mode().context("failed to enable raw mode for inline confirm")?;
    execute!(out, Hide).context("failed to hide cursor for inline confirm")?;
    execute!(out, SavePosition).context("failed to save cursor for inline confirm")?;

    let result = (|| -> Result<bool> {
        let mut yes_selected = true;
        loop {
            execute!(
                out,
                RestorePosition,
                MoveToColumn(0),
                Clear(ClearType::FromCursorDown)
            )
                .context("failed to clear inline confirm area")?;
            write!(out, "{prompt}  ").context("failed to write prompt")?;

            let yes = if yes_selected {
                format!("{} {}", "●".green().bold(), "Yes".green().bold())
            } else {
                format!("{} {}", "○".dark_grey(), "Yes")
            };
            let no = if yes_selected {
                format!("{} {}", "○".dark_grey(), "No")
            } else {
                format!("{} {}", "●".yellow().bold(), "No".yellow().bold())
            };
            write!(out, "{yes}   {no}").context("failed to write options")?;
            out.flush().context("failed to flush inline confirm")?;

            if let Event::Key(key) =
                event::read().context("failed to read key for inline confirm")?
            {
                match key.code {
                    KeyCode::Left | KeyCode::Up => yes_selected = true,
                    KeyCode::Right | KeyCode::Down => yes_selected = false,
                    KeyCode::Tab => yes_selected = !yes_selected,
                    KeyCode::Enter => return Ok(yes_selected),
                    KeyCode::Esc => return Err(UserCancelled.into()),
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Err(UserCancelled.into());
                    }
                    _ => {}
                }
            }
        }
    })();

    let _ = execute!(out, RestorePosition, Show, Clear(ClearType::FromCursorDown));
    let _ = disable_raw_mode();
    let _ = writeln!(out);
    result
}

fn build_branch_picker_items(
    ordered_names: &[String],
    current: &str,
    tracked: &[BranchRecord],
) -> Vec<String> {
    let tracked_map: HashMap<&str, &BranchRecord> =
        tracked.iter().map(|b| (b.name.as_str(), b)).collect();
    ordered_names
        .iter()
        .map(|name| {
            if name == current {
                format!("● current  {name}")
            } else if let Some(rec) = tracked_map.get(name.as_str()) {
                let pr = rec.cached_pr_state.as_deref().unwrap_or("none");
                format!("◆ tracked  {name}  (pr:{pr})")
            } else {
                format!("○ local    {name}")
            }
        })
        .collect()
}

fn build_delete_picker_items(
    tracked_names: &[String],
    current: &str,
    tracked: &[BranchRecord],
) -> Vec<String> {
    let tracked_map: HashMap<&str, &BranchRecord> =
        tracked.iter().map(|b| (b.name.as_str(), b)).collect();
    tracked_names
        .iter()
        .map(|name| {
            if name == current {
                format!("● current  {name}")
            } else if let Some(rec) = tracked_map.get(name.as_str()) {
                let pr = rec.cached_pr_state.as_deref().unwrap_or("none");
                format!("◆ tracked  {name}  (pr:{pr})")
            } else {
                format!("◆ tracked  {name}")
            }
        })
        .collect()
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
        assert!(got.downcast_ref::<UserCancelled>().is_some());
    }

    #[test]
    fn pr_metadata_parse_error_warning_is_user_friendly() {
        let err = anyhow!("expected value at line 1 column 1");
        let msg = format_pr_metadata_warning("feat/a", &err, false);
        assert!(msg.contains("gh returned an unexpected response"));
        assert!(!msg.contains("line 1 column 1"));
    }
}
