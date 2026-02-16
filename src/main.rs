mod cli;
mod core;
mod db;
mod git;
mod output;
mod provider;
mod tui;

use std::collections::{HashMap, HashSet};
use std::io::{IsTerminal, stdin, stdout};

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use dialoguer::{Confirm, Input, Select};
use output::{BranchView, DoctorIssueView, print_json};
use provider::Provider;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Commands};
use crate::core::{build_sync_plan, rank_parent_candidates, render_plain_tree};
use crate::db::{BranchRecord, Database};
use crate::git::Git;
use crate::provider::GithubProvider;

fn main() -> Result<()> {
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
    let provider = GithubProvider::new(git.clone());

    match cli.command {
        None => cmd_stack(&db, &git, cli.porcelain),
        Some(Commands::Create(args)) => {
            cmd_create(&db, &git, &args.parent, &args.name, cli.porcelain)
        }
        Some(Commands::Sync(args)) => cmd_sync(
            &db,
            &git,
            &provider,
            &base_branch,
            cli.porcelain,
            cli.yes,
            args.dry_run,
        ),
        Some(Commands::Doctor(args)) => cmd_doctor(&db, &git, cli.porcelain, args.fix),
        Some(Commands::Unlink(args)) => {
            cmd_unlink(&db, &args.branch, args.drop_record, cli.porcelain)
        }
    }
}

fn cmd_stack(db: &Database, git: &Git, porcelain: bool) -> Result<()> {
    let records = db.list_branches()?;
    let branch_views = to_branch_views(git, &records)?;

    if porcelain {
        return print_json(&branch_views);
    }

    if stdout().is_terminal() && stdin().is_terminal() {
        return tui::run_stack_tui(&branch_views);
    }

    println!("{}", render_plain_tree(&records));
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

    let parent = if let Some(parent) = parent_arg {
        parent.clone()
    } else if stdout().is_terminal() && stdin().is_terminal() {
        let default_idx = parent_candidates
            .iter()
            .position(|b| b == &current)
            .unwrap_or(0);
        let idx = Select::new()
            .with_prompt("Select parent branch")
            .items(&parent_candidates)
            .default(default_idx)
            .interact()?;
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
        Input::<String>::new()
            .with_prompt("Name for new child branch")
            .validate_with(|input: &String| -> Result<(), &str> {
                if input.trim().is_empty() {
                    Err("branch name cannot be empty")
                } else {
                    Ok(())
                }
            })
            .interact_text()?
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
    db.set_sync_sha(&child, &child_sha)?;
    let out = serde_json::json!({
        "created": child,
        "parent": parent,
        "head_sha": child_sha,
        "db": db_summary_path(git)?,
    });

    if porcelain {
        print_json(&out)?;
    } else {
        println!(
            "created stack branch: {} -> {}",
            out["parent"], out["created"]
        );
    }
    Ok(())
}

fn cmd_sync(
    db: &Database,
    git: &Git,
    provider: &dyn Provider,
    base_branch: &str,
    porcelain: bool,
    yes: bool,
    dry_run: bool,
) -> Result<()> {
    let plan = build_sync_plan(db, git, provider, base_branch)?;
    let plan_view = plan.to_view();

    if porcelain {
        print_json(&plan_view)?;
    } else {
        println!("sync base: {}", plan.base_branch);
        for op in &plan_view.operations {
            println!("- {}: {} {}", op.kind, op.branch, op.details);
        }
    }

    if dry_run {
        return Ok(());
    }

    let should_apply = if yes {
        true
    } else if stdout().is_terminal() && stdin().is_terminal() {
        Confirm::new()
            .with_prompt("Apply sync plan?")
            .default(false)
            .interact()?
    } else {
        false
    };

    if !should_apply {
        if !porcelain {
            println!("sync plan not applied");
        }
        return Ok(());
    }

    core::execute_sync_plan(db, git, &plan)?;
    if !porcelain {
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
        if let Some(pid) = branch.parent_branch_id {
            if !id_to_name.contains_key(&pid) {
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

fn cmd_unlink(db: &Database, branch: &str, drop_record: bool, porcelain: bool) -> Result<()> {
    if db.branch_by_name(branch)?.is_none() {
        return Err(anyhow!("branch '{}' is not tracked", branch));
    }

    if drop_record {
        db.delete_branch(branch)?;
    } else {
        db.clear_parent(branch)?;
    }

    let payload = serde_json::json!({
        "branch": branch,
        "drop_record": drop_record,
        "status": "ok"
    });

    if porcelain {
        print_json(&payload)?;
    } else if drop_record {
        println!("removed branch record '{branch}'");
    } else {
        println!("unlinked '{branch}' from parent");
    }

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
