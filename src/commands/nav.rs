use std::collections::{HashMap, HashSet};
use std::io::{IsTerminal, stdin, stdout};

use anyhow::{Context, Result, anyhow};
use dialoguer::{Select, theme::ColorfulTheme};

use crate::db::{BranchRecord, Database};
use crate::git::Git;
use crate::ui::interaction::prompt_or_cancel;

#[derive(Debug, Clone, Copy)]
pub enum NavCommand {
    Top,
    Bottom,
    Up,
    Down,
}

impl NavCommand {
    fn as_str(self) -> &'static str {
        match self {
            NavCommand::Top => "top",
            NavCommand::Bottom => "bottom",
            NavCommand::Up => "up",
            NavCommand::Down => "down",
        }
    }
}

pub fn run(db: &Database, git: &Git, command: NavCommand, porcelain: bool) -> Result<()> {
    let current = git.current_branch()?;
    if current.trim().is_empty() {
        return Err(anyhow!("cannot navigate stack from detached HEAD"));
    }

    let tracked = db.list_branches()?;
    let by_name: HashMap<&str, &BranchRecord> =
        tracked.iter().map(|b| (b.name.as_str(), b)).collect();
    let by_id: HashMap<i64, &BranchRecord> = tracked.iter().map(|b| (b.id, b)).collect();
    let mut children_by_parent: HashMap<i64, Vec<String>> = HashMap::new();
    for rec in &tracked {
        if let Some(parent_id) = rec.parent_branch_id {
            children_by_parent
                .entry(parent_id)
                .or_default()
                .push(rec.name.clone());
        }
    }

    let current_record = by_name.get(current.as_str()).copied().ok_or_else(|| {
        anyhow!(
            "current branch '{}' is not tracked; run `stack track` first",
            current
        )
    })?;

    let target = match command {
        NavCommand::Down => resolve_down(current_record, &by_id)?,
        NavCommand::Bottom => resolve_bottom(current_record, &by_id)?,
        NavCommand::Up => {
            let children = viable_children(git, &children_by_parent, current_record.id)?;
            choose_child(
                &current,
                &children,
                "Select child branch to switch to",
                porcelain,
            )?
        }
        NavCommand::Top => resolve_top(
            git,
            current_record,
            &by_name,
            &children_by_parent,
            porcelain,
        )?,
    };

    if !git.branch_exists(&target)? {
        return Err(anyhow!("target branch does not exist in git: {target}"));
    }

    let changed = target != current;
    if changed {
        git.checkout_branch(&target)
            .with_context(|| format!("failed to switch to branch '{target}'"))?;
    }

    if porcelain {
        crate::views::print_json(&serde_json::json!({
            "command": command.as_str(),
            "from": current,
            "to": target,
            "changed": changed,
        }))?;
    } else if changed {
        println!("switched: {} -> {}", current, target);
    } else {
        println!("already on {}", target);
    }

    Ok(())
}

fn resolve_down(current: &BranchRecord, by_id: &HashMap<i64, &BranchRecord>) -> Result<String> {
    let parent_id = current.parent_branch_id.ok_or_else(|| {
        anyhow!(
            "branch '{}' has no parent branch in the stack",
            current.name
        )
    })?;
    let parent = by_id
        .get(&parent_id)
        .ok_or_else(|| anyhow!("tracked parent metadata missing for '{}'", current.name))?;
    Ok(parent.name.clone())
}

fn resolve_bottom(current: &BranchRecord, by_id: &HashMap<i64, &BranchRecord>) -> Result<String> {
    let mut cursor = current;
    let mut seen = HashSet::new();
    seen.insert(cursor.id);

    while let Some(parent_id) = cursor.parent_branch_id {
        let parent = by_id
            .get(&parent_id)
            .copied()
            .ok_or_else(|| anyhow!("tracked parent metadata missing for '{}'", cursor.name))?;
        if !seen.insert(parent.id) {
            return Err(anyhow!("detected a cycle while walking stack parents"));
        }
        cursor = parent;
    }

    Ok(cursor.name.clone())
}

fn resolve_top(
    git: &Git,
    current: &BranchRecord,
    by_name: &HashMap<&str, &BranchRecord>,
    children_by_parent: &HashMap<i64, Vec<String>>,
    porcelain: bool,
) -> Result<String> {
    let mut cursor = current.name.clone();
    let mut cursor_id = current.id;
    let mut seen = HashSet::new();
    seen.insert(cursor_id);

    loop {
        let children = viable_children(git, children_by_parent, cursor_id)?;
        if children.is_empty() {
            return Ok(cursor);
        }
        let next = choose_child(
            &cursor,
            &children,
            "Select child branch to continue toward top",
            porcelain,
        )?;
        let next_id = by_name
            .get(next.as_str())
            .copied()
            .ok_or_else(|| anyhow!("tracked child metadata missing for '{}'", next))?
            .id;
        cursor = next;
        cursor_id = next_id;
        if !seen.insert(cursor_id) {
            return Err(anyhow!("detected a cycle while walking stack children"));
        }
    }
}

fn viable_children(
    git: &Git,
    children_by_parent: &HashMap<i64, Vec<String>>,
    parent_id: i64,
) -> Result<Vec<String>> {
    let mut out = Vec::new();
    if let Some(children) = children_by_parent.get(&parent_id) {
        for child in children {
            if git.branch_exists(child)? {
                out.push(child.clone());
            }
        }
    }
    Ok(out)
}

fn choose_child(
    current: &str,
    children: &[String],
    prompt: &str,
    porcelain: bool,
) -> Result<String> {
    if children.is_empty() {
        return Err(anyhow!(
            "branch '{}' has no child branches in the stack",
            current
        ));
    }
    if children.len() == 1 {
        return Ok(children[0].clone());
    }

    if !porcelain && stdout().is_terminal() && stdin().is_terminal() {
        let idx = prompt_or_cancel(
            Select::with_theme(&ColorfulTheme::default())
                .with_prompt(format!(
                    "{prompt} from '{current}' (↑/↓ to navigate, Enter to select, Ctrl-C to cancel)"
                ))
                .items(children)
                .default(0)
                .interact(),
        )?;
        return Ok(children[idx].clone());
    }

    Err(anyhow!(
        "branch '{}' has multiple child branches; run in interactive mode to choose one: {}",
        current,
        children.join(", ")
    ))
}
