use std::collections::HashMap;

use crossterm::style::Stylize;

use crate::db::BranchRecord;
use crate::util::url::{
    escape_markdown_link_label, url_encode_branch_path, url_encode_compare_ref,
    url_encode_component,
};

#[derive(Debug, Clone)]
pub struct BranchLinkTarget {
    pub base_url: String,
    pub head_ref: String,
}

pub fn render_tree(
    branches: &[BranchRecord],
    color: bool,
    pr_base_url: Option<&str>,
    default_base_branch: &str,
    link_targets: Option<&HashMap<String, BranchLinkTarget>>,
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
        link_targets: Option<&'a HashMap<String, BranchLinkTarget>>,
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
                    ctx.link_targets.and_then(|m| m.get(&node.name)),
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
        link_targets,
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
    link_target: Option<&BranchLinkTarget>,
    pr_number: Option<i64>,
    parent_branch: Option<&str>,
    child_branches: &[String],
    head_branch: &str,
    default_base_branch: &str,
    color: bool,
) -> String {
    let base = link_target.map(|t| t.base_url.as_str()).or(pr_base_url);
    let Some(base) = base else {
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
            url_encode_compare_ref(compare_base),
            url_encode_compare_ref(
                link_target
                    .map(|t| t.head_ref.as_str())
                    .unwrap_or(head_branch),
            ),
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
    let base_label = escape_markdown_link_label(base_branch);
    let base_path = url_encode_branch_path(base_branch);
    let head_label = escape_markdown_link_label(head_branch);
    let head_path = url_encode_branch_path(head_branch);
    let mut lines = vec!["### Stack Flow".to_string()];
    lines.push(format!(
        "[{base_label}]({root}/tree/{base_path}) -> [{head_label}]({root}/tree/{head_path})"
    ));
    if let Some(parent) = parent_branch {
        let parent_label = escape_markdown_link_label(parent);
        let parent_path = url_encode_branch_path(parent);
        lines.push(format!(
            "parent: [{parent_label}]({root}/tree/{parent_path})"
        ));
    }
    if !child_branches.is_empty() {
        let children = child_branches
            .iter()
            .map(|child| {
                let child_label = escape_markdown_link_label(child);
                let child_path = url_encode_branch_path(child);
                format!("[{child_label}]({root}/tree/{child_path})")
            })
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

        let rendered = render_tree(&branches, false, None, "main", None);
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

        let rendered = render_tree(&branches, true, None, "main", None);
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
            None,
        );
        assert!(rendered.contains("https://github.com/acme/repo/pull/42"));
    }

    #[test]
    fn render_tree_existing_pr_prefers_branch_link_target_repo() {
        let branches = vec![BranchRecord {
            id: 1,
            name: "feat/a".to_string(),
            parent_branch_id: None,
            last_synced_head_sha: Some("abc".to_string()),
            cached_pr_number: Some(42),
            cached_pr_state: Some("open".to_string()),
        }];
        let mut link_targets = HashMap::new();
        link_targets.insert(
            "feat/a".to_string(),
            BranchLinkTarget {
                base_url: "https://github.com/upstream/repo".to_string(),
                head_ref: "feat/a".to_string(),
            },
        );

        let rendered = render_tree(
            &branches,
            false,
            Some("https://github.com/fork/repo"),
            "main",
            Some(&link_targets),
        );
        assert!(rendered.contains("https://github.com/upstream/repo/pull/42"));
        assert!(!rendered.contains("https://github.com/fork/repo/pull/42"));
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
            None,
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
            None,
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
            None,
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
            None,
        );
        assert!(rendered.contains("[no PR (same base/head)]"));
        assert!(!rendered.contains("/compare/main...main"));
    }

    #[test]
    fn compose_stack_pr_body_escapes_labels_and_encodes_branch_paths() {
        let body = compose_stack_pr_body(
            "https://github.com/acme/repo",
            "main(prod)",
            "feat/[head)",
            Some("feat/paren]t"),
            &["child one".to_string()],
        );
        assert!(
            body.contains("[main\\(prod\\)](https://github.com/acme/repo/tree/main%28prod%29)")
        );
        assert!(
            body.contains("[feat/\\[head\\)](https://github.com/acme/repo/tree/feat/%5Bhead%29)")
        );
        assert!(body.contains(
            "parent: [feat/paren\\]t](https://github.com/acme/repo/tree/feat/paren%5Dt)"
        ));
        assert!(
            body.contains("children: [child one](https://github.com/acme/repo/tree/child%20one)")
        );
    }
}
