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
}

#[derive(Debug, Clone)]
pub struct GithubProvider {
    git: Git,
}

impl GithubProvider {
    pub fn new(git: Git) -> Self {
        Self { git }
    }

    fn run_gh(&self, args: &[&str]) -> Result<String> {
        let output = Command::new("gh")
            .current_dir(self.git.root())
            .args(args)
            .output()
            .with_context(|| format!("failed to run gh {args:?}"))?;
        if !output.status.success() {
            return Ok(String::new());
        }
        Ok(String::from_utf8(output.stdout)?)
    }
}

#[derive(Debug, Deserialize)]
struct GhPr {
    number: i64,
    state: String,
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
                "number,state,mergeCommit".to_string(),
            ]
        } else {
            vec![
                "pr".to_string(),
                "list".to_string(),
                "--head".to_string(),
                branch.to_string(),
                "--state".to_string(),
                "all".to_string(),
                "--json".to_string(),
                "number,state,mergeCommit".to_string(),
            ]
        };
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = self.run_gh(&arg_refs)?;
        if out.trim().is_empty() {
            return Ok(None);
        }

        if cached_number.is_some() {
            let pr: GhPr = serde_json::from_str(&out)?;
            return Ok(Some(convert_pr(pr)));
        }

        let mut prs: Vec<GhPr> = serde_json::from_str(&out)?;
        prs.sort_by_key(|p| p.number);
        Ok(prs.pop().map(convert_pr))
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
        let out = self.run_gh(&arg_refs)?;
        Ok(CreatePrResult {
            url: out.lines().last().unwrap_or_default().trim().to_string(),
        })
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
    }
}
