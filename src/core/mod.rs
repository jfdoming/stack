use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::{Result, anyhow};
use crossterm::style::Stylize;

use crate::db::{BranchRecord, Database};
use crate::git::{Git, StashHandle};
use crate::output::{OperationView, SyncPlanView};
use crate::provider::{PrState, Provider};
use crate::util::url::url_encode_component;

#[derive(Debug, Clone)]
pub enum SyncOp {
    Fetch {
        remote: String,
    },
    Restack {
        branch: String,
        onto: String,
        reason: String,
    },
    UpdateSha {
        branch: String,
        sha: String,
    },
}

#[derive(Debug, Clone)]
pub struct SyncPlan {
    pub base_branch: String,
    pub ops: Vec<SyncOp>,
}

impl SyncPlan {
    pub fn to_view(&self) -> SyncPlanView {
        let mut operations = Vec::new();
        for op in &self.ops {
            match op {
                SyncOp::Fetch { remote } => operations.push(OperationView {
                    kind: "fetch".to_string(),
                    branch: remote.clone(),
                    onto: None,
                    details: format!("fetch {remote}"),
                }),
                SyncOp::Restack {
                    branch,
                    onto,
                    reason,
                } => operations.push(OperationView {
                    kind: "restack".to_string(),
                    branch: branch.clone(),
                    onto: Some(onto.clone()),
                    details: format!("onto {onto}: {reason}"),
                }),
                SyncOp::UpdateSha { branch, sha } => operations.push(OperationView {
                    kind: "update_sha".to_string(),
                    branch: branch.clone(),
                    onto: None,
                    details: sha.clone(),
                }),
            }
        }
        SyncPlanView {
            base_branch: self.base_branch.clone(),
            operations,
        }
    }
}

pub fn rank_parent_candidates(
    current: &str,
    tracked: &[BranchRecord],
    local: &[String],
) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    push_unique(&mut out, &mut seen, current);

    for b in tracked {
        push_unique(&mut out, &mut seen, &b.name);
    }

    for b in local {
        push_unique(&mut out, &mut seen, b);
    }

    out
}

fn push_unique(out: &mut Vec<String>, seen: &mut HashSet<String>, value: &str) {
    if seen.insert(value.to_string()) {
        out.push(value.to_string());
    }
}

pub fn build_sync_plan(
    db: &Database,
    git: &Git,
    provider: &dyn Provider,
    base_branch: &str,
    base_remote: &str,
) -> Result<SyncPlan> {
    let tracked = db.list_branches()?;
    let mut ops = vec![SyncOp::Fetch {
        remote: base_remote.to_string(),
    }];
    let mut by_id: HashMap<i64, BranchRecord> = HashMap::new();
    let mut children: HashMap<i64, Vec<i64>> = HashMap::new();

    for b in &tracked {
        by_id.insert(b.id, b.clone());
        if let Some(parent) = b.parent_branch_id {
            children.entry(parent).or_default().push(b.id);
        }
    }

    let mut queue: VecDeque<(String, String)> = VecDeque::new();

    for branch in &tracked {
        if !git.branch_exists(&branch.name)? {
            continue;
        }

        if let Some(pr) = provider.resolve_pr_by_head(&branch.name, branch.cached_pr_number)? {
            let state = match pr.state {
                PrState::Open => "open",
                PrState::Merged => "merged",
                PrState::Closed => "closed",
                PrState::Unknown => "unknown",
            };
            db.set_pr_cache(&branch.name, Some(pr.number), Some(state))?;

            if matches!(pr.state, PrState::Merged) {
                let new_base = pr
                    .merge_commit_oid
                    .unwrap_or_else(|| format!("{base_remote}/{base_branch}"));
                if let Some(children_ids) = children.get(&branch.id) {
                    for child_id in children_ids {
                        if let Some(child) = by_id.get(child_id) {
                            queue.push_back((child.name.clone(), new_base.clone()));
                        }
                    }
                }
            }
        }

        let current_sha = git.head_sha(&branch.name)?;
        if let Some(previous_sha) = &branch.last_synced_head_sha
            && previous_sha != &current_sha
            && let Some(children_ids) = children.get(&branch.id)
        {
            for child_id in children_ids {
                if let Some(child) = by_id.get(child_id) {
                    queue.push_back((child.name.clone(), branch.name.clone()));
                }
            }
        }
        ops.push(SyncOp::UpdateSha {
            branch: branch.name.clone(),
            sha: current_sha,
        });
    }

    let mut seen_restack = HashSet::new();
    while let Some((branch, onto)) = queue.pop_front() {
        if !seen_restack.insert(branch.clone()) {
            continue;
        }
        ops.push(SyncOp::Restack {
            branch: branch.clone(),
            onto: onto.clone(),
            reason: "parent updated or merged".to_string(),
        });
        if let Some(node) = tracked.iter().find(|b| b.name == branch)
            && let Some(children_ids) = children.get(&node.id)
        {
            for child_id in children_ids {
                if let Some(child) = by_id.get(child_id) {
                    queue.push_back((child.name.clone(), branch.clone()));
                }
            }
        }
    }

    Ok(SyncPlan {
        base_branch: base_branch.to_string(),
        ops,
    })
}

pub fn execute_sync_plan(db: &Database, git: &Git, plan: &SyncPlan) -> Result<()> {
    let mut stash: Option<StashHandle> = None;
    if git.is_worktree_dirty()? {
        eprintln!("warning: worktree is dirty; auto-stashing local changes");
        stash = git.stash_push("stack-sync-auto-stash")?;
    }

    let run_id = db.record_sync_start()?;
    let mut status = "success";
    let mut summary = None;
    let replay_supported = git.supports_replay();

    let result: Result<()> = (|| {
        for op in &plan.ops {
            match op {
                SyncOp::Fetch { remote } => git.fetch_remote(remote)?,
                SyncOp::Restack { branch, onto, .. } => {
                    let old_base = git.merge_base(branch, onto)?;
                    if replay_supported {
                        if let Err(err) = git.replay_onto(branch, &old_base, onto) {
                            let reason = summarize_replay_error(&err);
                            eprintln!(
                                "warning: git replay is unavailable for '{branch}' ({reason}); falling back to rebase"
                            );
                            git.rebase_onto(branch, &old_base, onto)?;
                        }
                    } else {
                        eprintln!("warning: git replay unavailable; using rebase for {branch}");
                        git.rebase_onto(branch, &old_base, onto)?;
                    }
                    let sha = git.head_sha(branch)?;
                    db.set_sync_sha(branch, &sha)?;
                }
                SyncOp::UpdateSha { branch, sha } => db.set_sync_sha(branch, sha)?,
            }
        }
        Ok(())
    })();

    if let Some(stash_handle) = stash
        && let Err(err) = git.stash_pop(&stash_handle)
    {
        eprintln!(
            "warning: could not auto-restore stash {}: {err}",
            stash_handle.reference
        );
    }

    if let Err(err) = result {
        status = "failed";
        summary = Some(format!(
            "{{\"error\":{}}}",
            serde_json::to_string(&err.to_string())?
        ));
        db.record_sync_finish(run_id, status, summary.as_deref())?;
        return Err(anyhow!("sync failed: {err}"));
    }

    db.record_sync_finish(run_id, status, summary.as_deref())?;
    Ok(())
}

fn summarize_replay_error(err: &anyhow::Error) -> String {
    let msg = err.to_string();
    if msg.contains("replaying down to root commit is not supported yet") {
        return "cannot replay down to the root commit".to_string();
    }
    if msg.contains("git command failed") {
        return "git replay command failed".to_string();
    }
    msg
}

pub fn render_tree(
    branches: &[BranchRecord],
    color: bool,
    pr_base_url: Option<&str>,
    default_base_branch: &str,
) -> String {
    let mut out = String::new();
    let mut children: HashMap<Option<i64>, Vec<&BranchRecord>> = HashMap::new();
    let mut by_id: HashMap<i64, &BranchRecord> = HashMap::new();
    for b in branches {
        children.entry(b.parent_branch_id).or_default().push(b);
        by_id.insert(b.id, b);
    }
    for vals in children.values_mut() {
        vals.sort_by(|a, b| a.name.cmp(&b.name));
    }

    struct RenderCtx<'a> {
        children: &'a HashMap<Option<i64>, Vec<&'a BranchRecord>>,
        by_id: &'a HashMap<i64, &'a BranchRecord>,
        color: bool,
        pr_base_url: Option<&'a str>,
        default_base_branch: &'a str,
    }

    fn walk(out: &mut String, parent: Option<i64>, prefix: &str, ctx: &RenderCtx<'_>) {
        if let Some(nodes) = ctx.children.get(&parent) {
            for (idx, node) in nodes.iter().enumerate() {
                let is_last = idx + 1 == nodes.len();
                let connector = if is_last { "└──" } else { "├──" };
                let branch_name = if ctx.color {
                    node.name.as_str().green().bold().to_string()
                } else {
                    node.name.clone()
                };
                let pr = render_pr_state(node.cached_pr_state.as_deref(), ctx.color);
                let sync = render_sync_state(node.last_synced_head_sha.is_some(), ctx.color);
                let parent_name = node
                    .parent_branch_id
                    .and_then(|id| ctx.by_id.get(&id).map(|b| b.name.as_str()));
                let child_names: Vec<String> = ctx
                    .children
                    .get(&Some(node.id))
                    .map(|children| children.iter().map(|child| child.name.clone()).collect())
                    .unwrap_or_default();
                let pr_link = render_pr_link(
                    ctx.pr_base_url,
                    node.cached_pr_number,
                    parent_name,
                    &child_names,
                    &node.name,
                    ctx.default_base_branch,
                    ctx.color,
                );
                let mut line = format!("{prefix}{connector} {branch_name}");
                if let Some(pr) = pr {
                    line.push(' ');
                    line.push_str(&pr);
                }
                line.push(' ');
                line.push_str(&sync);
                line.push_str(&pr_link);
                out.push_str(&line);
                out.push('\n');
                let next_prefix = if is_last {
                    format!("{prefix}    ")
                } else {
                    format!("{prefix}│   ")
                };
                walk(out, Some(node.id), &next_prefix, ctx);
            }
        }
    }

    let ctx = RenderCtx {
        children: &children,
        by_id: &by_id,
        color,
        pr_base_url,
        default_base_branch,
    };
    walk(&mut out, None, "", &ctx);
    if out.is_empty() {
        out.push_str("(no stack branches tracked)\n");
    }
    out
}

fn render_pr_state(pr: Option<&str>, color: bool) -> Option<String> {
    let badge = match pr.unwrap_or("none") {
        "open" => "PR:open",
        "merged" => "PR:merged",
        "closed" => "PR:closed",
        "unknown" => "PR:unknown",
        _ => return None,
    };
    if !color {
        return Some(format!("[{badge}]"));
    }
    Some(match badge {
        "PR:open" => format!("[{}]", badge.yellow().bold()),
        "PR:merged" => format!("[{}]", badge.green().bold()),
        "PR:closed" => format!("[{}]", badge.red().bold()),
        "PR:unknown" => format!("[{}]", badge.dark_grey()),
        _ => format!("[{}]", badge.dark_grey()),
    })
}

fn render_sync_state(has_sha: bool, color: bool) -> String {
    let badge = if has_sha {
        "SYNC:tracked"
    } else {
        "SYNC:never"
    };
    if !color {
        return format!("[{badge}]");
    }
    if has_sha {
        format!("[{}]", badge.cyan())
    } else {
        format!("[{}]", badge.magenta())
    }
}

fn render_pr_link(
    pr_base_url: Option<&str>,
    pr_number: Option<i64>,
    parent_branch: Option<&str>,
    child_branches: &[String],
    head_branch: &str,
    default_base_branch: &str,
    color: bool,
) -> String {
    let Some(base) = pr_base_url else {
        return String::new();
    };
    let url = if let Some(number) = pr_number {
        format!("{}/pull/{}", base.trim_end_matches('/'), number)
    } else {
        let compare_base = parent_branch.unwrap_or(default_base_branch);
        if compare_base == head_branch {
            return if color {
                format!(" {}", "[no PR (same base/head)]".dark_grey())
            } else {
                " [no PR (same base/head)]".to_string()
            };
        }
        let body = compose_stack_pr_body(
            base,
            compare_base,
            head_branch,
            parent_branch,
            child_branches,
        );
        format!(
            "{}/compare/{}...{}?expand=1&body={}",
            base.trim_end_matches('/'),
            compare_base,
            head_branch,
            url_encode_component(&body)
        )
    };
    if color {
        let label = if let Some(number) = pr_number {
            format!("PR #{number}")
        } else {
            "[no PR]".to_string()
        };
        format!(" {}", osc8_hyperlink(&url, &label).dark_grey().underlined())
    } else if pr_number.is_some() {
        format!(" {url}")
    } else {
        format!(" [no PR] {url}")
    }
}

fn osc8_hyperlink(url: &str, label: &str) -> String {
    format!("\u{1b}]8;;{url}\u{1b}\\{label}\u{1b}]8;;\u{1b}\\")
}

fn compose_stack_pr_body(
    base_url: &str,
    base_branch: &str,
    head_branch: &str,
    parent_branch: Option<&str>,
    child_branches: &[String],
) -> String {
    let root = base_url.trim_end_matches('/');
    let mut lines = vec!["### Stack Flow".to_string()];
    lines.push(format!(
        "[{base_branch}]({root}/tree/{base_branch}) -> [{head_branch}]({root}/tree/{head_branch})"
    ));
    if let Some(parent) = parent_branch {
        lines.push(format!("parent: [{parent}]({root}/tree/{parent})"));
    }
    if !child_branches.is_empty() {
        let children = child_branches
            .iter()
            .map(|child| format!("[{child}]({root}/tree/{child})"))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("children: {children}"));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::BranchRecord;

    #[test]
    fn ranking_starts_with_current_then_tracked() {
        let tracked = vec![
            BranchRecord {
                id: 1,
                name: "feat/b".to_string(),
                parent_branch_id: None,
                last_synced_head_sha: None,
                cached_pr_number: None,
                cached_pr_state: None,
            },
            BranchRecord {
                id: 2,
                name: "feat/a".to_string(),
                parent_branch_id: None,
                last_synced_head_sha: None,
                cached_pr_number: None,
                cached_pr_state: None,
            },
        ];
        let local = vec![
            "main".to_string(),
            "feat/a".to_string(),
            "fix/c".to_string(),
        ];
        let ranked = rank_parent_candidates("feat/current", &tracked, &local);
        assert_eq!(ranked[0], "feat/current");
        assert_eq!(ranked[1], "feat/b");
        assert_eq!(ranked[2], "feat/a");
        assert!(ranked.contains(&"fix/c".to_string()));
    }

    #[test]
    fn render_tree_plain_includes_vertical_connectors_and_badges() {
        let branches = vec![
            BranchRecord {
                id: 1,
                name: "main".to_string(),
                parent_branch_id: None,
                last_synced_head_sha: Some("abc".to_string()),
                cached_pr_number: None,
                cached_pr_state: Some("open".to_string()),
            },
            BranchRecord {
                id: 2,
                name: "feat/a".to_string(),
                parent_branch_id: Some(1),
                last_synced_head_sha: None,
                cached_pr_number: None,
                cached_pr_state: Some("merged".to_string()),
            },
        ];

        let rendered = render_tree(&branches, false, None, "main");
        assert!(rendered.contains("└── feat/a"));
        assert!(rendered.contains("[PR:open]"));
        assert!(rendered.contains("[SYNC:never]"));
    }

    #[test]
    fn render_tree_colored_emits_ansi_sequences() {
        let branches = vec![BranchRecord {
            id: 1,
            name: "main".to_string(),
            parent_branch_id: None,
            last_synced_head_sha: Some("abc".to_string()),
            cached_pr_number: None,
            cached_pr_state: Some("open".to_string()),
        }];

        let rendered = render_tree(&branches, true, None, "main");
        assert!(rendered.contains("\u{1b}["));
    }

    #[test]
    fn render_tree_includes_pr_link_when_repo_url_known() {
        let branches = vec![BranchRecord {
            id: 1,
            name: "main".to_string(),
            parent_branch_id: None,
            last_synced_head_sha: Some("abc".to_string()),
            cached_pr_number: Some(42),
            cached_pr_state: Some("open".to_string()),
        }];

        let rendered = render_tree(
            &branches,
            false,
            Some("https://github.com/acme/repo"),
            "main",
        );
        assert!(rendered.contains("https://github.com/acme/repo/pull/42"));
    }

    #[test]
    fn render_tree_colored_encodes_clickable_pr_link() {
        let branches = vec![BranchRecord {
            id: 1,
            name: "feat/a".to_string(),
            parent_branch_id: None,
            last_synced_head_sha: None,
            cached_pr_number: Some(123),
            cached_pr_state: Some("open".to_string()),
        }];

        let rendered = render_tree(
            &branches,
            true,
            Some("https://github.com/acme/repo"),
            "main",
        );
        assert!(rendered.contains("\u{1b}]8;;https://github.com/acme/repo/pull/123\u{1b}\\"));
        assert!(rendered.contains("PR #123"));
    }

    #[test]
    fn render_tree_colored_uses_no_pr_label_for_compare_link() {
        let branches = vec![BranchRecord {
            id: 1,
            name: "feat/no-pr".to_string(),
            parent_branch_id: None,
            last_synced_head_sha: None,
            cached_pr_number: None,
            cached_pr_state: Some("none".to_string()),
        }];

        let rendered = render_tree(
            &branches,
            true,
            Some("https://github.com/acme/repo"),
            "main",
        );
        assert!(rendered.contains(
            "\u{1b}]8;;https://github.com/acme/repo/compare/main...feat/no-pr?expand=1&body="
        ));
        assert!(rendered.contains("[no PR]"));
    }

    #[test]
    fn render_tree_includes_compare_link_when_pr_missing() {
        let branches = vec![BranchRecord {
            id: 1,
            name: "feat/a".to_string(),
            parent_branch_id: None,
            last_synced_head_sha: Some("abc".to_string()),
            cached_pr_number: None,
            cached_pr_state: None,
        }];

        let rendered = render_tree(
            &branches,
            false,
            Some("https://github.com/acme/repo"),
            "main",
        );
        assert!(!rendered.contains("[PR:none]"));
        assert!(rendered.contains("[no PR]"));
        assert!(rendered.contains("https://github.com/acme/repo/compare/main...feat/a?expand=1"));
        assert!(rendered.contains("body=%23%23%23%20Stack%20Flow"));
    }

    #[test]
    fn render_tree_marks_same_base_head_without_broken_compare_link() {
        let branches = vec![BranchRecord {
            id: 1,
            name: "main".to_string(),
            parent_branch_id: None,
            last_synced_head_sha: Some("abc".to_string()),
            cached_pr_number: None,
            cached_pr_state: None,
        }];

        let rendered = render_tree(
            &branches,
            false,
            Some("https://github.com/acme/repo"),
            "main",
        );
        assert!(rendered.contains("[no PR (same base/head)]"));
        assert!(!rendered.contains("/compare/main...main"));
    }

    #[test]
    fn summarize_replay_error_root_commit_case_is_human_readable() {
        let err = anyhow!(
            "git command failed [\"replay\", \"--onto\", \"main\", \"abc\", \"feat\"]: fatal: replaying down to root commit is not supported yet!"
        );
        let got = summarize_replay_error(&err);
        assert_eq!(got, "cannot replay down to the root commit");
    }

    #[test]
    fn summarize_replay_error_generic_git_failure_is_simplified() {
        let err = anyhow!("git command failed [\"replay\"]: fatal: something broke");
        let got = summarize_replay_error(&err);
        assert_eq!(got, "git replay command failed");
    }
}
