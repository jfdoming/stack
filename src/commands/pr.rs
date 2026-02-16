use std::collections::HashMap;
use std::io::{IsTerminal, stdout};
use std::process::Command;

use anyhow::{Context, Result, anyhow};
use crossterm::style::Stylize;

use crate::args::PrArgs;
use crate::db::{BranchRecord, Database};
use crate::git::Git;
use crate::provider::Provider;
use crate::util::pr_body::{ManagedBranchRef, compose_branch_pr_body};
use crate::util::pr_links::determine_pr_link_target;
use crate::util::terminal::osc8_hyperlink;
use crate::util::url::url_encode_component;

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
    _yes: bool,
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
        })?;
        let mut children: Vec<BranchPrRef> = records
            .iter()
            .filter(|r| r.parent_branch_id == Some(record.id))
            .map(|r| BranchPrRef {
                branch: r.name.clone(),
                pr_number: r.cached_pr_number,
            })
            .collect();
        children.sort_by(|a, b| a.branch.cmp(&b.branch));
        Some(ManagedPrSection {
            parent: Some(parent),
            children,
        })
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
            let pr_ref = format_existing_pr_ref(git, &base, &current, number)?;
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
        let pr_ref = format_existing_pr_ref(git, &base, &current, number)?;
        println!(
            "PR already exists for '{}': {}",
            payload["head"].as_str().unwrap_or_default(),
            pr_ref
        );
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
            let use_clickable = stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
            println!("{}", format_manual_pr_link(&url, use_clickable));
        }
    }
    Ok(())
}

fn format_manual_pr_link(url: &str, use_clickable: bool) -> String {
    if use_clickable {
        return osc8_hyperlink(url, "open PR manually")
            .underlined()
            .to_string();
    }
    format!("open PR manually: {url}")
}

fn format_existing_pr_ref(git: &Git, base_branch: &str, head_branch: &str, number: i64) -> Result<String> {
    let label = format!("#{number}");
    let use_clickable = stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    if !use_clickable {
        return Ok(label);
    }

    let Ok(link_target) = determine_pr_link_target(git, base_branch, head_branch) else {
        return Ok(label);
    };
    let url = format!("{}/pull/{}", link_target.base_url.trim_end_matches('/'), number);
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
    let link_target = determine_pr_link_target(git, base, head)?;
    let base_url = link_target.base_url;
    let head_ref = link_target.head_ref;
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
    let parent = managed
        .and_then(|m| m.parent.as_ref())
        .map(|p| ManagedBranchRef {
            branch: p.branch.clone(),
            pr_number: p.pr_number,
            pr_url: None,
        });
    let first_child = managed
        .and_then(|m| m.children.first())
        .map(|c| ManagedBranchRef {
            branch: c.branch.clone(),
            pr_number: c.pr_number,
            pr_url: None,
        });
    Some(compose_branch_pr_body(
        base_url,
        base_branch,
        parent.as_ref(),
        first_child.as_ref(),
        user_body,
    ))
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
        assert!(body.contains(crate::util::pr_body::MANAGED_BODY_MARKER_START));
        assert!(body.contains(crate::util::pr_body::MANAGED_BODY_MARKER_END));
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
        assert!(body.contains(crate::util::pr_body::MANAGED_BODY_MARKER_START));
        assert!(body.contains(crate::util::pr_body::MANAGED_BODY_MARKER_END));
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

    #[test]
    fn format_manual_pr_link_is_clickable_when_supported() {
        let url = "https://github.com/acme/repo/pull/1";
        let out = format_manual_pr_link(url, true);
        assert!(out.contains("\u{1b}]8;;https://github.com/acme/repo/pull/1\u{1b}\\"));
        assert!(out.contains("open PR manually"));
    }

    #[test]
    fn format_manual_pr_link_plain_keeps_full_url() {
        let url = "https://github.com/acme/repo/compare/main...very/long/branch/name?expand=1";
        let out = format_manual_pr_link(url, false);
        assert_eq!(out, format!("open PR manually: {url}"));
    }
}
