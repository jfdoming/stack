use std::collections::{HashMap, HashSet};
use std::io::{IsTerminal, stdin, stdout};

use anyhow::{Context, Result, anyhow};
use dialoguer::{Input, MultiSelect, theme::ColorfulTheme};

use crate::args::SplitArgs;
use crate::db::{BranchRecord, Database, ParentUpdate};
use crate::git::Git;
use crate::ui::interaction::{confirm_inline_yes_no, prompt_or_cancel};

const SPLIT_SEMANTICS: &str = "selected commits become tips of new lower stack branches; commits after them appear above; the current branch remains the top branch";

#[derive(Debug, Clone)]
struct SplitPoint {
    commit: String,
    name: String,
}

#[derive(Debug, Clone)]
struct TopBranch {
    old_name: String,
    name: String,
}

pub fn run(
    db: &Database,
    git: &Git,
    args: &SplitArgs,
    porcelain: bool,
    yes: bool,
    base_branch: &str,
) -> Result<()> {
    let current = git.current_branch()?;
    if current.is_empty() {
        return Err(anyhow!("cannot split while HEAD is detached"));
    }
    if current == base_branch {
        return Err(anyhow!("cannot split base branch '{base_branch}'"));
    }

    let tracked = db.list_branches()?;
    let parent = resolve_parent(db, args.parent.as_deref(), &current, base_branch, &tracked)?;
    validate_parent(git, &parent, &current)?;

    let commits = git.rev_list_reverse(&parent, &current)?;
    if commits.len() < 2 {
        return Err(anyhow!(
            "split requires at least two commits in {parent}..{current}"
        ));
    }
    if git.has_merge_commits(&parent, &current)? {
        return Err(anyhow!(
            "split currently supports linear histories; rebase or flatten '{}' before running stack split",
            current
        ));
    }

    let head_sha = git.head_sha(&current)?;
    let split_points = resolve_split_points(args, git, &current, &parent, &commits, &head_sha)?;
    let top = resolve_top_branch(args, git, &current, split_points.len())?;
    if top.name != current && db.branch_by_name(&top.name)?.is_some() {
        return Err(anyhow!("top branch is already tracked: {}", top.name));
    }
    validate_branch_names(git, &split_points, &top)?;

    let top_parent = split_points
        .last()
        .map(|point| point.name.clone())
        .unwrap_or_else(|| parent.clone());
    let payload = payload(
        &current,
        &parent,
        &split_points,
        &top,
        &top_parent,
        !args.dry_run,
    );

    if !porcelain {
        print_plan(git, &parent, &split_points, &top, &commits)?;
    }

    if !args.dry_run && !yes {
        if !(stdout().is_terminal() && stdin().is_terminal()) {
            return Err(anyhow!(
                "split requires confirmation; rerun with --yes or --dry-run"
            ));
        }
        if !confirm_inline_yes_no("Apply this split?")? {
            println!("split not applied: confirmation declined; no changes made");
            return Ok(());
        }
    }

    if !args.dry_run {
        for point in &split_points {
            git.create_branch_from(&point.name, &point.commit)
                .with_context(|| format!("failed to create branch {}", point.name))?;
        }

        if top.name != current {
            git.rename_local_branch(&current, &top.name)
                .with_context(|| {
                    format!("failed to rename top branch {} to {}", current, top.name)
                })?;
            if db.branch_by_name(&current)?.is_some() {
                db.rename_branch(&current, &top.name)?;
            }
        }

        let mut updates = Vec::new();
        let mut previous = parent.clone();
        for point in &split_points {
            updates.push(ParentUpdate {
                child_name: point.name.clone(),
                parent_name: Some(previous),
            });
            previous = point.name.clone();
        }
        updates.push(ParentUpdate {
            child_name: top.name.clone(),
            parent_name: Some(previous),
        });
        db.set_parents_batch(&updates)?;

        for point in &split_points {
            db.set_sync_sha(&point.name, &point.commit)?;
        }
        db.set_sync_sha(&top.name, &head_sha)?;
    }

    if porcelain {
        crate::views::print_json(&payload)?;
    } else if args.dry_run {
        println!("split dry-run: no changes made");
    } else {
        println!("split '{}' into stack", current);
    }

    Ok(())
}

fn resolve_top_branch(
    args: &SplitArgs,
    git: &Git,
    current: &str,
    split_count: usize,
) -> Result<TopBranch> {
    let name = if let Some(top_name) = &args.top_name {
        top_name.trim().to_string()
    } else if stdout().is_terminal() && stdin().is_terminal() && args.at.is_empty() {
        let theme = ColorfulTheme::default();
        let mut used_names = HashSet::new();
        let default_name =
            default_split_branch_name(git, current, split_count + 1, &mut used_names)?;
        prompt_or_cancel(
            Input::<String>::with_theme(&theme)
                .with_prompt("Name top branch")
                .default(default_name)
                .validate_with(|input: &String| -> Result<(), &str> {
                    if input.trim().is_empty() {
                        Err("branch name cannot be empty")
                    } else {
                        Ok(())
                    }
                })
                .interact_text(),
        )?
        .trim()
        .to_string()
    } else {
        current.to_string()
    };

    if name.is_empty() {
        return Err(anyhow!("top branch name cannot be empty"));
    }
    if name != current {
        if !git.is_valid_branch_name(&name)? {
            return Err(anyhow!("invalid top branch name: {name}"));
        }
        if git.branch_exists(&name)? {
            return Err(anyhow!("top branch already exists: {name}"));
        }
    }

    Ok(TopBranch {
        old_name: current.to_string(),
        name,
    })
}

fn resolve_parent(
    db: &Database,
    parent_arg: Option<&str>,
    current: &str,
    base_branch: &str,
    tracked: &[BranchRecord],
) -> Result<String> {
    if let Some(parent) = parent_arg {
        return Ok(parent.trim().to_string());
    }

    let by_id: HashMap<i64, &BranchRecord> =
        tracked.iter().map(|branch| (branch.id, branch)).collect();
    if let Some(current_record) = db.branch_by_name(current)?
        && let Some(parent_id) = current_record.parent_branch_id
        && let Some(parent) = by_id.get(&parent_id)
    {
        return Ok(parent.name.clone());
    }

    Ok(base_branch.to_string())
}

fn validate_parent(git: &Git, parent: &str, current: &str) -> Result<()> {
    if parent.trim().is_empty() {
        return Err(anyhow!("parent branch name cannot be empty"));
    }
    if !git.branch_exists(parent)? {
        return Err(anyhow!("parent branch does not exist in git: {parent}"));
    }
    if !git.is_ancestor(parent, current)? {
        return Err(anyhow!(
            "parent branch '{parent}' is not an ancestor of '{current}'"
        ));
    }
    Ok(())
}

fn resolve_split_points(
    args: &SplitArgs,
    git: &Git,
    current: &str,
    parent: &str,
    commits: &[String],
    head_sha: &str,
) -> Result<Vec<SplitPoint>> {
    let explicit_points = !args.at.is_empty();
    if explicit_points && args.at.len() != args.name.len() {
        return Err(anyhow!(
            "each --at commit requires exactly one --name branch"
        ));
    }
    if !explicit_points && !args.name.is_empty() {
        return Err(anyhow!("--name requires a matching --at commit"));
    }

    let names_by_commit = if explicit_points {
        explicit_split_names(args, git, parent, current, commits, head_sha)?
    } else {
        prompt_split_names(git, current, commits, head_sha)?
    };

    let mut split_points = Vec::new();
    for commit in commits {
        if let Some(name) = names_by_commit.get(commit) {
            split_points.push(SplitPoint {
                commit: commit.clone(),
                name: name.clone(),
            });
        }
    }
    Ok(split_points)
}

fn explicit_split_names(
    args: &SplitArgs,
    git: &Git,
    parent: &str,
    current: &str,
    commits: &[String],
    head_sha: &str,
) -> Result<HashMap<String, String>> {
    let commit_set: HashSet<&str> = commits.iter().map(String::as_str).collect();
    let mut names_by_commit = HashMap::new();
    for (raw_commit, name) in args.at.iter().zip(args.name.iter()) {
        let commit = git
            .resolve_commit(raw_commit)
            .with_context(|| format!("split commit does not resolve: {raw_commit}"))?;
        if commit == head_sha {
            return Err(anyhow!("cannot split at HEAD; choose an earlier commit"));
        }
        if !commit_set.contains(commit.as_str()) {
            return Err(anyhow!(
                "split commit '{}' is outside {}..{}",
                raw_commit,
                parent,
                current
            ));
        }
        if names_by_commit
            .insert(commit.clone(), name.trim().to_string())
            .is_some()
        {
            return Err(anyhow!("duplicate split commit: {raw_commit}"));
        }
    }
    Ok(names_by_commit)
}

fn prompt_split_names(
    git: &Git,
    current: &str,
    commits: &[String],
    head_sha: &str,
) -> Result<HashMap<String, String>> {
    if !(stdout().is_terminal() && stdin().is_terminal()) {
        return Err(anyhow!(
            "missing --at commits; {SPLIT_SEMANTICS}. Pass --at <commit> --name <branch>."
        ));
    }

    let selectable: Vec<String> = commits
        .iter()
        .filter(|commit| commit.as_str() != head_sha)
        .cloned()
        .collect();
    if selectable.is_empty() {
        return Err(anyhow!(
            "no split commits available; split requires at least one commit before HEAD"
        ));
    }

    let theme = ColorfulTheme::default();
    let labels = selectable
        .iter()
        .map(|commit| git.commit_oneline(commit))
        .collect::<Result<Vec<_>>>()?;
    let selected = prompt_or_cancel(
        MultiSelect::with_theme(&theme)
            .with_prompt("Select top commits for lower branches")
            .items(&labels)
            .interact(),
    )?;
    if selected.is_empty() {
        return Err(anyhow!("no split commits selected"));
    }

    let mut names_by_commit = HashMap::new();
    let mut default_names = HashSet::new();
    for (idx, selected_idx) in selected.iter().enumerate() {
        let commit = selectable[*selected_idx].clone();
        let default_name = default_split_branch_name(git, current, idx + 1, &mut default_names)?;
        let name = prompt_or_cancel(
            Input::<String>::with_theme(&theme)
                .with_prompt(format!("Branch name for {}", labels[*selected_idx]))
                .default(default_name)
                .validate_with(|input: &String| -> Result<(), &str> {
                    if input.trim().is_empty() {
                        Err("branch name cannot be empty")
                    } else {
                        Ok(())
                    }
                })
                .interact_text(),
        )?;
        names_by_commit.insert(commit, name.trim().to_string());
    }

    Ok(names_by_commit)
}

fn default_split_branch_name(
    git: &Git,
    current: &str,
    part_number: usize,
    used: &mut HashSet<String>,
) -> Result<String> {
    let base = format!("{current}-part-{part_number}");
    let mut candidate = base.clone();
    let mut suffix = 2;
    while used.contains(&candidate) || git.branch_exists(&candidate)? {
        candidate = format!("{base}-{suffix}");
        suffix += 1;
    }
    used.insert(candidate.clone());
    Ok(candidate)
}

fn validate_branch_names(git: &Git, split_points: &[SplitPoint], top: &TopBranch) -> Result<()> {
    let mut seen = HashSet::new();
    for point in split_points {
        if point.name.is_empty() {
            return Err(anyhow!("branch name cannot be empty"));
        }
        if !seen.insert(point.name.clone()) {
            return Err(anyhow!("duplicate branch name: {}", point.name));
        }
        if !git.is_valid_branch_name(&point.name)? {
            return Err(anyhow!("invalid branch name: {}", point.name));
        }
        if git.branch_exists(&point.name)? {
            return Err(anyhow!("branch already exists: {}", point.name));
        }
    }
    if top.name != top.old_name && !seen.insert(top.name.clone()) {
        return Err(anyhow!("duplicate branch name: {}", top.name));
    }
    Ok(())
}

fn print_plan(
    git: &Git,
    parent: &str,
    split_points: &[SplitPoint],
    top: &TopBranch,
    commits: &[String],
) -> Result<()> {
    let commits_by_branch = commits_by_branch(split_points, &top.name, commits);

    println!("Planned stack:");
    println!("  {parent}");
    for point in split_points {
        println!("  -> {}", point.name);
        print_branch_commits(
            git,
            commits_by_branch
                .get(point.name.as_str())
                .unwrap_or(&Vec::new()),
        )?;
    }
    if top.name == top.old_name {
        println!("  -> {}", top.name);
    } else {
        println!("  -> {} (renames {})", top.name, top.old_name);
    }
    print_branch_commits(
        git,
        commits_by_branch
            .get(top.name.as_str())
            .unwrap_or(&Vec::new()),
    )?;

    Ok(())
}

fn commits_by_branch<'a>(
    split_points: &'a [SplitPoint],
    top_name: &'a str,
    commits: &'a [String],
) -> HashMap<&'a str, Vec<&'a str>> {
    let mut split_by_commit = HashMap::new();
    for point in split_points {
        split_by_commit.insert(point.commit.as_str(), point.name.as_str());
    }

    let mut out: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut current_branch = split_points
        .first()
        .map(|point| point.name.as_str())
        .unwrap_or(top_name);
    for commit in commits {
        out.entry(current_branch).or_default().push(commit.as_str());
        if split_by_commit.contains_key(commit.as_str()) {
            current_branch = split_points
                .iter()
                .position(|point| point.name == current_branch)
                .and_then(|idx| split_points.get(idx + 1))
                .map(|point| point.name.as_str())
                .unwrap_or(top_name);
        }
    }
    out
}

fn print_branch_commits(git: &Git, commits: &[&str]) -> Result<()> {
    for commit in commits {
        println!("     {}", git.commit_oneline(commit)?);
    }
    Ok(())
}

fn payload(
    current: &str,
    parent: &str,
    split_points: &[SplitPoint],
    top: &TopBranch,
    top_parent: &str,
    applied: bool,
) -> serde_json::Value {
    let splits = split_points
        .iter()
        .map(|point| {
            serde_json::json!({
                "name": point.name,
                "commit": point.commit,
            })
        })
        .collect::<Vec<_>>();

    serde_json::json!({
        "action": "split",
        "applied": applied,
        "current": current,
        "parent": parent,
        "splits": splits,
        "top": {
            "old_name": top.old_name,
            "name": top.name,
            "renamed": top.name != top.old_name,
        },
        "top_parent": top_parent,
    })
}
