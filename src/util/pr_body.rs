use crate::util::url::{escape_markdown_link_label, url_encode_branch_path};

#[derive(Debug, Clone)]
pub struct ManagedBranchRef {
    pub branch: String,
    pub pr_number: Option<i64>,
    pub pr_url: Option<String>,
}

pub const MANAGED_BODY_MARKER_START: &str = "<!-- stack:managed:start -->";
pub const MANAGED_BODY_MARKER_END: &str = "<!-- stack:managed:end -->";

pub fn managed_pr_section(
    base_url: &str,
    base_branch: &str,
    base_commit_url: Option<&str>,
    parent: Option<&ManagedBranchRef>,
    first_child: Option<&ManagedBranchRef>,
) -> String {
    let root = base_url.trim_end_matches('/');
    let base_label = escape_markdown_link_label(base_branch);
    let base_path = url_encode_branch_path(base_branch);
    let parent_chain = parent
        .map(|p| {
            if p.branch == base_branch {
                base_commit_url
                    .map(|url| format!("[{base_label}]({url})"))
                    .unwrap_or_else(|| format_pr_chain_node(root, p))
            } else {
                format_pr_chain_node(root, p)
            }
        })
        .unwrap_or_else(|| {
            base_commit_url
                .map(|url| format!("[{base_label}]({url})"))
                .unwrap_or_else(|| format!("[{base_label}]({root}/tree/{base_path})"))
        });
    let prefix = if parent.is_some_and(|p| p.branch != base_branch) {
        "… → ".to_string()
    } else {
        String::new()
    };
    let managed_line = if let Some(child) = first_child {
        format!(
            "{prefix}{parent_chain} → (this PR) → {} → …",
            format_pr_chain_node(root, child)
        )
    } else {
        format!("{prefix}{parent_chain} → (this PR)")
    };
    format!("{MANAGED_BODY_MARKER_START}\n{managed_line}\n<hr />\n{MANAGED_BODY_MARKER_END}")
}

pub fn compose_branch_pr_body(
    base_url: &str,
    base_branch: &str,
    base_commit_url: Option<&str>,
    parent: Option<&ManagedBranchRef>,
    first_child: Option<&ManagedBranchRef>,
    user_body: Option<&str>,
) -> String {
    let managed_section =
        managed_pr_section(base_url, base_branch, base_commit_url, parent, first_child);
    let user = user_body.and_then(|body| {
        let trimmed = body.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    });
    if let Some(user) = user {
        format!("{managed_section}\n\n{user}")
    } else {
        managed_section
    }
}

pub fn merge_managed_pr_section(existing_body: Option<&str>, managed_section: &str) -> String {
    let existing = existing_body.and_then(|b| {
        let trimmed = b.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    });

    let Some(existing) = existing else {
        return managed_section.to_string();
    };

    if let Some((start, end)) = managed_section_bounds(existing) {
        let prefix = existing[..start].trim_end();
        let suffix = existing[end..].trim_start();
        return match (prefix.is_empty(), suffix.is_empty()) {
            (true, true) => managed_section.to_string(),
            (true, false) => format!("{managed_section}\n\n{suffix}"),
            (false, true) => format!("{prefix}\n\n{managed_section}"),
            (false, false) => format!("{prefix}\n\n{managed_section}\n\n{suffix}"),
        };
    }

    format!("{managed_section}\n\n{existing}")
}

fn managed_section_bounds(body: &str) -> Option<(usize, usize)> {
    let start = body.find(MANAGED_BODY_MARKER_START)?;
    let end_start = body[start..].find(MANAGED_BODY_MARKER_END)? + start;
    let end = end_start + MANAGED_BODY_MARKER_END.len();
    Some((start, end))
}

fn format_pr_chain_node(root: &str, node: &ManagedBranchRef) -> String {
    if let Some(number) = node.pr_number {
        if let Some(url) = node.pr_url.as_deref() {
            format!("[#{number}]({url})")
        } else {
            format!("[#{number}]({root}/pull/{number})")
        }
    } else {
        let label = escape_markdown_link_label(&node.branch);
        let encoded = url_encode_branch_path(&node.branch);
        format!("[{label}]({root}/tree/{encoded})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_pr_section_wraps_stack_flow_in_markers() {
        let parent = ManagedBranchRef {
            branch: "feat/parent".to_string(),
            pr_number: Some(12),
            pr_url: None,
        };
        let child = ManagedBranchRef {
            branch: "feat/child".to_string(),
            pr_number: None,
            pr_url: None,
        };
        let body = managed_pr_section(
            "https://github.com/acme/repo",
            "main",
            None,
            Some(&parent),
            Some(&child),
        );
        assert!(body.contains(MANAGED_BODY_MARKER_START));
        assert!(body.contains(MANAGED_BODY_MARKER_END));
        assert!(body.contains("[#12](https://github.com/acme/repo/pull/12)"));
        assert!(body.contains("[feat/child](https://github.com/acme/repo/tree/feat/child)"));
        assert!(body.contains("… → [#12]"));
        assert!(body.contains("→ (this PR) →"));
    }

    #[test]
    fn managed_pr_section_base_parent_has_no_leading_ellipsis() {
        let body = managed_pr_section("https://github.com/acme/repo", "main", None, None, None);
        assert!(body.contains("[main](https://github.com/acme/repo/tree/main) → (this PR)"));
        assert!(!body.contains("… [main]"));
    }

    #[test]
    fn managed_pr_section_base_parent_with_child_has_no_leading_ellipsis() {
        let base_parent = ManagedBranchRef {
            branch: "main".to_string(),
            pr_number: None,
            pr_url: None,
        };
        let child = ManagedBranchRef {
            branch: "feat/next".to_string(),
            pr_number: Some(6693),
            pr_url: None,
        };
        let body = managed_pr_section(
            "https://github.com/acme/repo",
            "main",
            None,
            Some(&base_parent),
            Some(&child),
        );
        assert!(
            body.contains(
                "[main](https://github.com/acme/repo/tree/main) → (this PR) → [#6693](https://github.com/acme/repo/pull/6693) → …"
            )
        );
        assert!(!body.contains("… → [main]"));
    }

    #[test]
    fn managed_pr_section_last_branch_has_no_trailing_ellipsis() {
        let parent = ManagedBranchRef {
            branch: "feat/parent".to_string(),
            pr_number: Some(12),
            pr_url: None,
        };
        let body = managed_pr_section(
            "https://github.com/acme/repo",
            "main",
            None,
            Some(&parent),
            None,
        );
        assert!(body.contains("… → [#12](https://github.com/acme/repo/pull/12) → (this PR)"));
        assert!(!body.contains("(this PR) …"));
    }

    #[test]
    fn compose_branch_pr_body_appends_user_text_after_managed_block() {
        let body = compose_branch_pr_body(
            "https://github.com/acme/repo",
            "main",
            None,
            None,
            None,
            Some("details"),
        );
        assert!(body.starts_with(MANAGED_BODY_MARKER_START));
        assert!(body.contains(MANAGED_BODY_MARKER_END));
        assert!(body.ends_with("details"));
    }

    #[test]
    fn merge_managed_section_replaces_existing_managed_block() {
        let old =
            format!("{MANAGED_BODY_MARKER_START}\nold\n{MANAGED_BODY_MARKER_END}\n\nuser text");
        let new_section = format!("{MANAGED_BODY_MARKER_START}\nnew\n{MANAGED_BODY_MARKER_END}");
        let merged = merge_managed_pr_section(Some(&old), &new_section);
        assert_eq!(merged, format!("{new_section}\n\nuser text"));
    }

    #[test]
    fn merge_managed_section_prepends_when_markers_absent() {
        let new_section = format!("{MANAGED_BODY_MARKER_START}\nnew\n{MANAGED_BODY_MARKER_END}");
        let merged = merge_managed_pr_section(Some("user text"), &new_section);
        assert_eq!(merged, format!("{new_section}\n\nuser text"));
    }

    #[test]
    fn managed_pr_section_uses_base_commit_link_when_provided() {
        let body = managed_pr_section(
            "https://github.com/acme/repo",
            "main",
            Some("https://github.com/acme/repo/commit/abc123"),
            None,
            None,
        );
        assert!(body.contains("[main](https://github.com/acme/repo/commit/abc123)"));
        assert!(!body.contains("/tree/main"));
    }

    #[test]
    fn managed_pr_section_escapes_labels_and_encodes_branch_paths() {
        let parent = ManagedBranchRef {
            branch: "feat/paren]t".to_string(),
            pr_number: None,
            pr_url: None,
        };
        let child = ManagedBranchRef {
            branch: "feat/[child)".to_string(),
            pr_number: None,
            pr_url: None,
        };
        let body = managed_pr_section(
            "https://github.com/acme/repo",
            "main(prod)",
            None,
            Some(&parent),
            Some(&child),
        );
        assert!(
            body.contains("[feat/paren\\]t](https://github.com/acme/repo/tree/feat/paren%5Dt)")
        );
        assert!(
            body.contains("[feat/\\[child\\)](https://github.com/acme/repo/tree/feat/%5Bchild%29)")
        );
    }
}
