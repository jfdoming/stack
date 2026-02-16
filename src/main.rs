mod cli;
mod commands;
mod core;
mod db;
mod git;
mod output;
mod provider;
mod tui;
mod util;

use std::collections::{HashMap, HashSet};
use std::io::{IsTerminal, stdin, stdout};
use std::process::Command;

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use crossterm::style::Stylize;
use dialoguer::{Select, theme::ColorfulTheme};
use output::{BranchView, print_json};
use provider::Provider;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Commands, DeleteArgs, PrArgs, TrackArgs};
use crate::commands::shared::UserCancelled;
use crate::core::{rank_parent_candidates, render_tree};
use crate::db::{BranchRecord, Database, ParentUpdate};
use crate::git::Git;
use crate::provider::GithubProvider;

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

#[derive(Debug, Clone)]
struct ManagedPrSection {
    parent: Option<BranchPrRef>,
    children: Vec<BranchPrRef>,
}

#[derive(Debug, Clone)]
struct BranchPrRef {
    branch: String,
    pr_number: Option<i64>,
}

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
    let managed_pr_section = current_record.and_then(|record| {
        let parent = record.parent_branch_id.and_then(|parent_id| {
            by_id.get(&parent_id).map(|r| BranchPrRef {
                branch: r.name.clone(),
                pr_number: r.cached_pr_number,
            })
        });
        if parent.is_none() {
            return None;
        }
        let mut children: Vec<BranchPrRef> = records
            .iter()
            .filter(|r| r.parent_branch_id == Some(record.id))
            .map(|r| BranchPrRef {
                branch: r.name.clone(),
                pr_number: r.cached_pr_number,
            })
            .collect();
        children.sort_by(|a, b| a.branch.cmp(&b.branch));
        Some(ManagedPrSection { parent, children })
    });

    if current == base {
        let reason = format!(
            "cannot open PR from '{}' into itself; switch to a non-base branch or set a different parent/base",
            current
        );
        if porcelain {
            return print_json(&serde_json::json!({
                "head": current,
                "base": base,
                "can_open_link": false,
                "error": reason,
            }));
        }
        return Err(anyhow!(reason));
    }

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
        "will_open_link": existing.is_none(),
    });

    if args.dry_run {
        if porcelain {
            return print_json(&payload);
        }
        if let Some(number) = payload["existing_pr_number"].as_i64() {
            let pr_ref = format_existing_pr_ref(git, &base, number)?;
            println!(
                "PR already exists for '{}': {}",
                payload["head"].as_str().unwrap_or_default(),
                pr_ref
            );
        } else {
            println!(
                "would push '{}' and open a PR link with base={}",
                payload["head"], payload["base"]
            );
        }
        return Ok(());
    }

    if let Some(number) = payload["existing_pr_number"].as_i64() {
        if porcelain {
            return print_json(&payload);
        }
        let pr_ref = format_existing_pr_ref(git, &base, number)?;
        println!(
            "PR already exists for '{}': {}",
            payload["head"].as_str().unwrap_or_default(),
            pr_ref
        );
        return Ok(());
    }

    let should_open = if yes {
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

    if !should_open {
        if !porcelain {
            println!("PR open cancelled: confirmation declined; no changes made");
        }
        return Ok(());
    }

    let head = payload["head"].as_str().unwrap_or_default();
    let base_ref = payload["base"].as_str().unwrap_or_default();
    let push_remote = git
        .remote_for_branch(head)?
        .or_else(|| git.remote_for_branch(base_ref).ok().flatten())
        .unwrap_or_else(|| "origin".to_string());
    git.push_branch(&push_remote, head)?;
    let url = build_pr_open_url(
        git,
        base_ref,
        head,
        args.title.as_deref(),
        args.body.as_deref(),
        args.draft,
        managed_pr_section.as_ref(),
    )?;

    if porcelain {
        return print_json(&serde_json::json!({
            "head": payload["head"],
            "base": payload["base"],
            "push_remote": push_remote,
            "url": url,
        }));
    }

    println!("pushed '{head}' to '{push_remote}'");
    match open_url_in_browser(&url) {
        Ok(()) => println!("opened PR URL in browser"),
        Err(err) => {
            eprintln!("warning: could not auto-open PR URL ({err})");
            println!("open PR manually: {}", truncate_for_display(&url, 88));
        }
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

fn prompt_or_cancel<T>(result: dialoguer::Result<T>) -> Result<T> {
    commands::shared::prompt_or_cancel(result)
}

fn osc8_hyperlink(url: &str, label: &str) -> String {
    util::terminal::osc8_hyperlink(url, label)
}

fn format_existing_pr_ref(git: &Git, base_branch: &str, number: i64) -> Result<String> {
    let label = format!("#{number}");
    let use_clickable = stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    if !use_clickable {
        return Ok(label);
    }

    let Some(remote) = git.remote_for_branch(base_branch)? else {
        return Ok(label);
    };
    let Some(base_url) = git.remote_web_url(&remote)? else {
        return Ok(label);
    };
    let url = format!("{}/pull/{}", base_url.trim_end_matches('/'), number);
    Ok(osc8_hyperlink(&url, &label).underlined().to_string())
}

fn build_pr_open_url(
    git: &Git,
    base: &str,
    head: &str,
    title: Option<&str>,
    body: Option<&str>,
    draft: bool,
    managed: Option<&ManagedPrSection>,
) -> Result<String> {
    if base == head {
        return Err(anyhow!(
            "cannot build PR link when base and head are the same branch ('{}')",
            head
        ));
    }
    let head_remote = git
        .remote_for_branch(head)?
        .unwrap_or_else(|| "origin".to_string());
    let head_url = git.remote_web_url(&head_remote)?;
    let mut base_remote = git
        .remote_for_branch(base)?
        .or_else(|| git.remote_for_branch(head).ok().flatten())
        .unwrap_or_else(|| "origin".to_string());
    if let (Some(head_url), Some(upstream_url)) = (
        head_url.as_deref(),
        git.remote_web_url("upstream")?.as_deref(),
    ) && let (Some(head_owner), Some(upstream_owner)) = (
        github_owner_from_web_url(head_url),
        github_owner_from_web_url(upstream_url),
    ) && head_owner != upstream_owner
    {
        base_remote = "upstream".to_string();
    }

    let Some(base_url) = git.remote_web_url(&base_remote)? else {
        return Err(anyhow!(
            "unable to derive PR URL from remote '{}'; configure a GitHub-style remote URL",
            base_remote
        ));
    };

    let head_ref = if let (Some(head_url), Some(base_owner)) =
        (head_url.as_deref(), github_owner_from_web_url(&base_url))
        && let Some(head_owner) = github_owner_from_web_url(head_url)
    {
        if head_owner != base_owner {
            format!("{head_owner}:{head}")
        } else {
            head.to_string()
        }
    } else {
        head.to_string()
    };
    let mut params = vec!["expand=1".to_string()];
    if let Some(title) = title
        && !title.is_empty()
    {
        params.push(format!("title={}", url_encode_component(title)));
    }
    if let Some(body) = compose_pr_body(&base_url, base, head, managed, body).as_deref()
        && !body.is_empty()
    {
        params.push(format!("body={}", url_encode_component(body)));
    }
    if draft {
        params.push("draft=1".to_string());
    }
    Ok(format!(
        "{}/compare/{}...{}?{}",
        base_url.trim_end_matches('/'),
        base,
        head_ref,
        params.join("&")
    ))
}

fn url_encode_component(value: &str) -> String {
    util::url::url_encode_component(value)
}

fn open_url_in_browser(url: &str) -> Result<()> {
    if std::env::var("STACK_MOCK_BROWSER_OPEN").ok().as_deref() == Some("1") {
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };

    let output = cmd
        .output()
        .with_context(|| format!("failed to launch browser opener for URL '{}'", url))?;
    if !output.status.success() {
        return Err(anyhow!(
            "browser opener exited non-zero: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

fn github_owner_from_web_url(url: &str) -> Option<String> {
    util::url::github_owner_from_web_url(url)
}

fn compose_pr_body(
    base_url: &str,
    base_branch: &str,
    _head_branch: &str,
    managed: Option<&ManagedPrSection>,
    user_body: Option<&str>,
) -> Option<String> {
    let user_body = user_body.and_then(|b| {
        if b.trim().is_empty() {
            None
        } else {
            Some(b.trim())
        }
    });

    let root = base_url.trim_end_matches('/');
    let parent_chain = managed
        .and_then(|m| m.parent.as_ref())
        .map(|p| format_pr_chain_node(root, p))
        .unwrap_or_else(|| format!("[{base_branch}]({root}/tree/{base_branch})"));
    let first_child = managed
        .and_then(|m| m.children.first())
        .map(|c| format_pr_chain_node(root, c));

    let managed_line = if let Some(child) = first_child {
        format!("… {parent_chain} → #this PR (this PR) → {child} …")
    } else {
        format!("… {parent_chain} → #this PR (this PR) …")
    };

    Some(if let Some(user) = user_body {
        format!("{managed_line}\n\n{user}")
    } else {
        managed_line
    })
}

fn format_pr_chain_node(root: &str, node: &BranchPrRef) -> String {
    if let Some(number) = node.pr_number {
        format!("[#{number}]({root}/pull/{number})")
    } else {
        format!("[{}]({root}/tree/{})", node.branch, node.branch)
    }
}

fn truncate_for_display(value: &str, max_chars: usize) -> String {
    util::terminal::truncate_for_display(value, max_chars)
}

fn confirm_inline_yes_no(prompt: &str) -> Result<bool> {
    commands::shared::confirm_inline_yes_no(prompt)
}

fn should_use_inline_confirm(prompt: &str) -> Result<bool> {
    commands::shared::should_use_inline_confirm(prompt)
}

fn confirm_select_yes_no(prompt: &str) -> Result<bool> {
    commands::shared::confirm_select_yes_no(prompt)
}

fn build_branch_picker_items(
    ordered_names: &[String],
    current: &str,
    tracked: &[BranchRecord],
) -> Vec<String> {
    commands::shared::build_branch_picker_items(ordered_names, current, tracked)
}

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
        assert!(got.downcast_ref::<UserCancelled>().is_some());
    }

    #[test]
    fn pr_metadata_parse_error_warning_is_user_friendly() {
        let err = anyhow!("expected value at line 1 column 1");
        let msg = format_pr_metadata_warning("feat/a", &err, false);
        assert!(msg.contains("gh returned an unexpected response"));
        assert!(!msg.contains("line 1 column 1"));
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

    #[test]
    fn compose_pr_body_prepends_managed_section() {
        let managed = ManagedPrSection {
            parent: Some(BranchPrRef {
                branch: "feat/parent".to_string(),
                pr_number: Some(123),
            }),
            children: vec![
                BranchPrRef {
                    branch: "feat/child-a".to_string(),
                    pr_number: Some(125),
                },
                BranchPrRef {
                    branch: "feat/child-b".to_string(),
                    pr_number: None,
                },
            ],
        };
        let body = compose_pr_body(
            "https://github.com/acme/repo",
            "feat/base",
            "feat/head",
            Some(&managed),
            Some("User body text"),
        )
        .expect("body should be present");
        assert!(body.contains(
            "… [#123](https://github.com/acme/repo/pull/123) → #this PR (this PR) → [#125](https://github.com/acme/repo/pull/125) …"
        ));
        assert!(body.ends_with("User body text"));
    }

    #[test]
    fn compose_pr_body_returns_user_body_when_unmanaged() {
        let body = compose_pr_body(
            "https://github.com/acme/repo",
            "main",
            "feat/demo",
            None,
            Some("User body text"),
        )
        .expect("body should be present");
        assert!(
            body.contains(
                "… [main](https://github.com/acme/repo/tree/main) → #this PR (this PR) …"
            )
        );
        assert!(body.ends_with("User body text"));
    }

    #[test]
    fn compose_pr_body_omits_trailing_arrow_when_no_child_pr() {
        let managed = ManagedPrSection {
            parent: Some(BranchPrRef {
                branch: "feat/parent".to_string(),
                pr_number: Some(123),
            }),
            children: Vec::new(),
        };
        let body = compose_pr_body(
            "https://github.com/acme/repo",
            "feat/base",
            "feat/head",
            Some(&managed),
            None,
        )
        .expect("body should be present");
        assert!(
            body.contains("… [#123](https://github.com/acme/repo/pull/123) → #this PR (this PR) …")
        );
        assert!(!body.contains("#this PR (this PR) →"));
    }
}
