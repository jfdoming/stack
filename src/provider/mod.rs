use std::process::Command;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::git::Git;

#[derive(Debug, Clone)]
pub enum PrState {
    Open,
    Merged,
    Closed,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct PrInfo {
    pub number: i64,
    pub state: PrState,
    pub merge_commit_oid: Option<String>,
    pub base_ref_name: Option<String>,
}

pub trait Provider {
    fn resolve_pr_by_head(
        &self,
        branch: &str,
        cached_number: Option<i64>,
    ) -> Result<Option<PrInfo>>;
    fn delete_pr(&self, pr_number: i64) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct GithubProvider {
    git: Git,
    debug: bool,
}

impl GithubProvider {
    pub fn new(git: Git, debug: bool) -> Self {
        Self { git, debug }
    }

    fn run_gh_required(&self, args: &[&str]) -> Result<String> {
        let output = Command::new("gh")
            .current_dir(self.git.root())
            .env("NO_COLOR", "1")
            .env("CLICOLOR", "0")
            .args(args)
            .output()
            .with_context(|| format!("failed to run gh {args:?}"))?;
        if !output.status.success() {
            return Err(anyhow::anyhow!(
                "gh command failed {:?}: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(String::from_utf8(output.stdout)?)
    }

    fn run_gh_optional(&self, args: &[&str]) -> Result<Option<String>> {
        let output = Command::new("gh")
            .current_dir(self.git.root())
            .env("NO_COLOR", "1")
            .env("CLICOLOR", "0")
            .args(args)
            .output()
            .with_context(|| format!("failed to run gh {args:?}"))?;
        if !output.status.success() {
            eprintln!(
                "warning: gh command failed {:?}: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
            return Ok(None);
        }
        Ok(Some(String::from_utf8(output.stdout)?))
    }
}

#[derive(Debug, Deserialize)]
struct GhPr {
    number: i64,
    state: String,
    #[serde(rename = "baseRefName")]
    base_ref_name: Option<String>,
    #[serde(rename = "mergeCommit")]
    merge_commit: Option<GhMergeCommit>,
}

#[derive(Debug, Deserialize)]
struct GhMergeCommit {
    oid: String,
}

impl Provider for GithubProvider {
    fn resolve_pr_by_head(
        &self,
        branch: &str,
        cached_number: Option<i64>,
    ) -> Result<Option<PrInfo>> {
        let args: Vec<String> = if let Some(num) = cached_number {
            vec![
                "pr".to_string(),
                "view".to_string(),
                num.to_string(),
                "--json".to_string(),
                "number,state,mergeCommit,baseRefName".to_string(),
            ]
        } else {
            Vec::new()
        };

        if cached_number.is_some() {
            let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
            let Some(out) = self.run_gh_optional(&arg_refs)? else {
                return Ok(None);
            };
            if out.trim().is_empty() {
                return Ok(None);
            }
            let cleaned = clean_gh_json_output(&out);
            let pr: GhPr = serde_json::from_str(&cleaned).map_err(|err| {
                if self.debug {
                    anyhow::anyhow!(
                        "failed to parse gh PR metadata JSON: {err}; gh output: {}",
                        out.trim()
                    )
                } else {
                    err.into()
                }
            })?;
            return Ok(Some(convert_pr(pr)));
        }

        let mut head_filters = vec![branch.to_string()];
        if let Some(remote) = self.git.remote_for_branch(branch)?
            && let Some(url) = self.git.remote_web_url(&remote)?
            && let Some(owner) = github_owner_from_web_url(&url)
        {
            let qualified = format!("{owner}:{branch}");
            if !head_filters.iter().any(|h| h == &qualified) {
                head_filters.push(qualified);
            }
        }

        for head_filter in head_filters {
            let args = vec![
                "pr".to_string(),
                "list".to_string(),
                "--head".to_string(),
                head_filter,
                "--state".to_string(),
                "all".to_string(),
                "--json".to_string(),
                "number,state,mergeCommit,baseRefName".to_string(),
            ];
            let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
            let Some(out) = self.run_gh_optional(&arg_refs)? else {
                continue;
            };
            if out.trim().is_empty() {
                continue;
            }
            let cleaned = clean_gh_json_output(&out);
            let prs: Vec<GhPr> = serde_json::from_str(&cleaned).map_err(|err| {
                if self.debug {
                    anyhow::anyhow!(
                        "failed to parse gh PR list JSON for --head {}: {err}; gh output: {}",
                        args[3],
                        out.trim()
                    )
                } else {
                    err.into()
                }
            })?;
            if let Some(pr) = select_preferred_pr(prs) {
                return Ok(Some(convert_pr(pr)));
            }
        }
        Ok(None)
    }

    fn delete_pr(&self, pr_number: i64) -> Result<()> {
        let num = pr_number.to_string();
        let args = ["pr", "close", &num, "--delete-branch"];
        let _ = self.run_gh_required(&args)?;
        Ok(())
    }
}

fn convert_pr(pr: GhPr) -> PrInfo {
    let state = match pr.state.as_str() {
        "OPEN" => PrState::Open,
        "MERGED" => PrState::Merged,
        "CLOSED" => PrState::Closed,
        _ => PrState::Unknown,
    };
    PrInfo {
        number: pr.number,
        state,
        merge_commit_oid: pr.merge_commit.map(|m| m.oid),
        base_ref_name: pr.base_ref_name,
    }
}

fn select_preferred_pr(prs: Vec<GhPr>) -> Option<GhPr> {
    let mut best_open: Option<GhPr> = None;
    let mut best_any: Option<GhPr> = None;

    for pr in prs {
        if best_any.as_ref().is_none_or(|b| pr.number > b.number) {
            best_any = Some(GhPr {
                number: pr.number,
                state: pr.state.clone(),
                base_ref_name: pr.base_ref_name.clone(),
                merge_commit: pr
                    .merge_commit
                    .as_ref()
                    .map(|m| GhMergeCommit { oid: m.oid.clone() }),
            });
        }

        if pr.state == "OPEN" && best_open.as_ref().is_none_or(|b| pr.number > b.number) {
            best_open = Some(pr);
        }
    }

    best_open.or(best_any)
}

fn clean_gh_json_output(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                let _ = chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        if ch.is_control() && ch != '\n' && ch != '\r' && ch != '\t' {
            continue;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_gh_json_output_strips_ansi_and_controls() {
        let raw = "\u{1b}[32m[\n{\"number\":1,\"state\":\"OPEN\",\"baseRefName\":\"main\",\"mergeCommit\":null}\n]\u{1b}[0m";
        let cleaned = clean_gh_json_output(raw);
        assert!(cleaned.starts_with("["));
        assert!(cleaned.contains("\"number\":1"));
    }

    #[test]
    fn select_preferred_pr_prefers_open_over_higher_closed_number() {
        let prs = vec![
            GhPr {
                number: 6995,
                state: "CLOSED".to_string(),
                base_ref_name: Some("master".to_string()),
                merge_commit: None,
            },
            GhPr {
                number: 6693,
                state: "OPEN".to_string(),
                base_ref_name: Some("feature/base".to_string()),
                merge_commit: None,
            },
        ];
        let picked = select_preferred_pr(prs).expect("selected pr");
        assert_eq!(picked.number, 6693);
        assert_eq!(picked.state, "OPEN");
    }
}

fn github_owner_from_web_url(url: &str) -> Option<String> {
    let trimmed = url.trim_end_matches('/');
    let (_, rest) = trimmed.split_once("://")?;
    let mut parts = rest.split('/');
    let _host = parts.next()?;
    let owner = parts.next()?;
    if owner.is_empty() {
        return None;
    }
    Some(owner.to_string())
}
