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

#[derive(Debug, Clone)]
pub struct CreatePrRequest<'a> {
    pub head: &'a str,
    pub base: &'a str,
    pub title: Option<&'a str>,
    pub body: Option<&'a str>,
    pub draft: bool,
}

#[derive(Debug, Clone)]
pub struct CreatePrResult {
    pub url: String,
}

pub trait Provider {
    fn resolve_pr_by_head(
        &self,
        branch: &str,
        cached_number: Option<i64>,
    ) -> Result<Option<PrInfo>>;
    fn create_pr(&self, req: CreatePrRequest<'_>) -> Result<CreatePrResult>;
    fn delete_pr(&self, pr_number: i64) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct GithubProvider {
    git: Git,
}

impl GithubProvider {
    pub fn new(git: Git) -> Self {
        Self { git }
    }

    fn run_gh_required(&self, args: &[&str]) -> Result<String> {
        let output = Command::new("gh")
            .current_dir(self.git.root())
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
            let pr: GhPr = serde_json::from_str(&out)?;
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
            let mut prs: Vec<GhPr> = serde_json::from_str(&out)?;
            prs.sort_by_key(|p| p.number);
            if let Some(pr) = prs.pop() {
                return Ok(Some(convert_pr(pr)));
            }
        }
        Ok(None)
    }

    fn create_pr(&self, req: CreatePrRequest<'_>) -> Result<CreatePrResult> {
        let mut args = vec![
            "pr".to_string(),
            "create".to_string(),
            "--head".to_string(),
            req.head.to_string(),
            "--base".to_string(),
            req.base.to_string(),
        ];

        if req.draft {
            args.push("--draft".to_string());
        }

        match (req.title, req.body) {
            (Some(title), Some(body)) => {
                args.push("--title".to_string());
                args.push(title.to_string());
                args.push("--body".to_string());
                args.push(body.to_string());
            }
            (Some(title), None) => {
                args.push("--title".to_string());
                args.push(title.to_string());
                args.push("--fill".to_string());
            }
            _ => args.push("--fill".to_string()),
        }

        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = self.run_gh_required(&arg_refs)?;
        Ok(CreatePrResult {
            url: out.lines().last().unwrap_or_default().trim().to_string(),
        })
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
