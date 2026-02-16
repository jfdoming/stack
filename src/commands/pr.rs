use std::collections::HashMap;
use std::io::{IsTerminal, stdin, stdout};
use std::process::Command;

use anyhow::{Context, Result, anyhow};
use crossterm::style::Stylize;

use crate::args::PrArgs;
use crate::ui::interaction::confirm_inline_yes_no;
use crate::db::{BranchRecord, Database};
use crate::git::Git;
use crate::provider::Provider;
use crate::util::terminal::{osc8_hyperlink, truncate_for_display};
use crate::util::url::{github_owner_from_web_url, url_encode_component};

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

pub fn run(
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
            return crate::views::print_json(&serde_json::json!({
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
            return crate::views::print_json(&payload);
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
            return crate::views::print_json(&payload);
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
        return crate::views::print_json(&serde_json::json!({
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

#[cfg(test)]
mod tests {
    use super::*;

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
