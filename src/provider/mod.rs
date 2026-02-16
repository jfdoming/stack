use std::collections::{HashMap, HashSet};
use std::process::Command;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::git::Git;
use crate::util::url::{github_owner_from_web_url, github_repo_slug_from_web_url};

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
    pub body: Option<String>,
    pub url: Option<String>,
}

pub trait Provider {
    fn resolve_pr_by_head(
        &self,
        branch: &str,
        cached_number: Option<i64>,
    ) -> Result<Option<PrInfo>>;
    fn resolve_prs_by_head(
        &self,
        branches: &[(&str, Option<i64>)],
    ) -> Result<HashMap<String, PrInfo>> {
        let mut out = HashMap::new();
        for (branch, cached_number) in branches {
            if let Some(pr) = self.resolve_pr_by_head(branch, *cached_number)? {
                out.insert((*branch).to_string(), pr);
            }
        }
        Ok(out)
    }
    fn update_pr_body(&self, pr_number: i64, body: &str) -> Result<()>;
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

    fn repo_slug_for_remote(&self, remote: &str) -> Result<Option<String>> {
        Ok(self
            .git
            .remote_web_url(remote)?
            .and_then(|url| github_repo_slug_from_web_url(&url)))
    }

    fn repo_scope_candidates_for_branch(&self, branch: &str) -> Result<Vec<String>> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();

        if let Some(remote) = self.git.remote_for_branch(branch)?
            && let Some(slug) = self.repo_slug_for_remote(&remote)?
            && seen.insert(slug.clone())
        {
            out.push(slug);
        }
        for remote in ["upstream", "origin"] {
            if let Some(slug) = self.repo_slug_for_remote(remote)?
                && seen.insert(slug.clone())
            {
                out.push(slug);
            }
        }

        Ok(out)
    }

    fn repo_scope_candidates_for_branches(
        &self,
        branches: &[(&str, Option<i64>)],
    ) -> Result<Vec<String>> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();

        for remote in ["upstream", "origin"] {
            if let Some(slug) = self.repo_slug_for_remote(remote)?
                && seen.insert(slug.clone())
            {
                out.push(slug);
            }
        }

        for (branch, _) in branches {
            if let Some(remote) = self.git.remote_for_branch(branch)?
                && let Some(slug) = self.repo_slug_for_remote(&remote)?
                && seen.insert(slug.clone())
            {
                out.push(slug);
            }
        }

        Ok(out)
    }

    fn parse_gh_pr_list(&self, raw: &str, context: &str) -> Result<Vec<GhPr>> {
        let cleaned = clean_gh_json_output(raw);
        serde_json::from_str::<Vec<GhPr>>(&cleaned).map_err(|err| {
            if self.debug {
                anyhow::anyhow!(
                    "failed to parse gh PR list JSON for {}: {err}; gh output: {}",
                    context,
                    raw.trim()
                )
            } else {
                err.into()
            }
        })
    }

    fn parse_gh_pr_view(&self, raw: &str, context: &str) -> Result<GhPr> {
        let cleaned = clean_gh_json_output(raw);
        serde_json::from_str(&cleaned).map_err(|err| {
            if self.debug {
                anyhow::anyhow!(
                    "failed to parse gh PR metadata JSON for {}: {err}; gh output: {}",
                    context,
                    raw.trim()
                )
            } else {
                err.into()
            }
        })
    }
}

#[derive(Debug, Deserialize, Clone)]
struct GhPr {
    number: i64,
    state: String,
    #[serde(rename = "baseRefName")]
    base_ref_name: Option<String>,
    #[serde(rename = "headRefName")]
    head_ref_name: Option<String>,
    #[serde(rename = "headRepositoryOwner")]
    head_repository_owner: Option<GhOwner>,
    body: Option<String>,
    url: Option<String>,
    #[serde(rename = "mergeCommit")]
    merge_commit: Option<GhMergeCommit>,
}

#[derive(Debug, Deserialize, Clone)]
struct GhOwner {
    login: String,
}

#[derive(Debug, Deserialize, Clone)]
struct GhMergeCommit {
    oid: String,
}

impl Provider for GithubProvider {
    fn resolve_prs_by_head(
        &self,
        branches: &[(&str, Option<i64>)],
    ) -> Result<HashMap<String, PrInfo>> {
        let mut out = HashMap::new();
        if branches.is_empty() {
            return Ok(out);
        }

        let mut by_head: HashMap<String, Vec<GhPr>> = HashMap::new();
        let mut repo_scopes: Vec<Option<String>> = vec![None];
        for scope in self.repo_scope_candidates_for_branches(branches)? {
            repo_scopes.push(Some(scope));
        }
        for scope in repo_scopes {
            let mut args = vec![
                "pr".to_string(),
                "list".to_string(),
                "--state".to_string(),
                "all".to_string(),
                "--limit".to_string(),
                "200".to_string(),
                "--json".to_string(),
                "number,state,mergeCommit,baseRefName,headRefName,headRepositoryOwner,url,body"
                    .to_string(),
            ];
            if let Some(scope) = scope.as_deref() {
                args.push("--repo".to_string());
                args.push(scope.to_string());
            }
            let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
            let Some(raw) = self.run_gh_optional(&arg_refs)? else {
                continue;
            };
            if raw.trim().is_empty() {
                continue;
            }
            let context = scope.as_deref().unwrap_or("default");
            let prs = self.parse_gh_pr_list(&raw, context)?;
            for pr in prs {
                if let Some(head) = pr.head_ref_name.as_deref()
                    && !head.is_empty()
                {
                    by_head.entry(head.to_string()).or_default().push(pr);
                }
            }
        }

        for (branch, cached_number) in branches {
            let preferred_owner = self
                .git
                .remote_for_branch(branch)?
                .and_then(|remote| self.git.remote_web_url(&remote).ok().flatten())
                .and_then(|url| github_owner_from_web_url(&url));

            if let Some(candidates) = by_head.get(*branch) {
                let filtered = if let Some(owner) = preferred_owner.as_deref() {
                    let scoped: Vec<GhPr> = candidates
                        .iter()
                        .filter(|pr| {
                            pr.head_repository_owner
                                .as_ref()
                                .map(|o| o.login.eq_ignore_ascii_case(owner))
                                .unwrap_or(false)
                        })
                        .cloned()
                        .collect();
                    if scoped.is_empty() {
                        candidates.clone()
                    } else {
                        scoped
                    }
                } else {
                    candidates.clone()
                };

                if let Some(pr) = select_preferred_pr(filtered) {
                    let converted = convert_pr(pr);
                    if cached_number.is_none_or(|cached| cached == converted.number) {
                        out.insert((*branch).to_string(), converted);
                        continue;
                    }
                }
            }

            if cached_number.is_some()
                && let Some(pr) = self.resolve_pr_by_head(branch, *cached_number)?
            {
                out.insert((*branch).to_string(), pr);
            }
        }

        Ok(out)
    }

    fn resolve_pr_by_head(
        &self,
        branch: &str,
        cached_number: Option<i64>,
    ) -> Result<Option<PrInfo>> {
        if let Some(num) = cached_number {
            let mut scopes: Vec<Option<String>> = vec![None];
            for scope in self.repo_scope_candidates_for_branch(branch)? {
                scopes.push(Some(scope));
            }
            for scope in scopes {
                let mut args = vec![
                    "pr".to_string(),
                    "view".to_string(),
                    num.to_string(),
                    "--json".to_string(),
                    "number,state,mergeCommit,baseRefName,url,body".to_string(),
                ];
                if let Some(scope) = scope.as_deref() {
                    args.push("--repo".to_string());
                    args.push(scope.to_string());
                }
                let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
                let Some(out) = self.run_gh_optional(&arg_refs)? else {
                    continue;
                };
                if out.trim().is_empty() {
                    continue;
                }
                let context = scope.as_deref().unwrap_or("default");
                let pr = self.parse_gh_pr_view(&out, context)?;
                return Ok(Some(convert_pr(pr)));
            }
            return Ok(None);
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

        let mut scopes: Vec<Option<String>> = vec![None];
        for scope in self.repo_scope_candidates_for_branch(branch)? {
            scopes.push(Some(scope));
        }
        for scope in scopes {
            for head_filter in &head_filters {
                let mut args = vec![
                    "pr".to_string(),
                    "list".to_string(),
                    "--head".to_string(),
                    head_filter.to_string(),
                    "--state".to_string(),
                    "all".to_string(),
                    "--json".to_string(),
                    "number,state,mergeCommit,baseRefName,url,body".to_string(),
                ];
                if let Some(scope) = scope.as_deref() {
                    args.push("--repo".to_string());
                    args.push(scope.to_string());
                }
                let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
                let Some(out) = self.run_gh_optional(&arg_refs)? else {
                    continue;
                };
                if out.trim().is_empty() {
                    continue;
                }
                let context = format!(
                    "--head {} {}",
                    head_filter,
                    scope
                        .as_deref()
                        .map(|s| format!("--repo {s}"))
                        .unwrap_or_default()
                );
                let prs = self.parse_gh_pr_list(&out, &context)?;
                if let Some(pr) = select_preferred_pr(prs) {
                    return Ok(Some(convert_pr(pr)));
                }
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

    fn update_pr_body(&self, pr_number: i64, body: &str) -> Result<()> {
        let num = pr_number.to_string();
        let args = ["pr", "edit", &num, "--body", body];
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
        body: pr.body,
        url: pr.url,
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
                head_ref_name: pr.head_ref_name.clone(),
                head_repository_owner: pr.head_repository_owner.clone(),
                body: pr.body.clone(),
                url: pr.url.clone(),
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
                head_ref_name: Some("feature/top".to_string()),
                head_repository_owner: None,
                body: None,
                url: None,
                merge_commit: None,
            },
            GhPr {
                number: 6693,
                state: "OPEN".to_string(),
                base_ref_name: Some("feature/base".to_string()),
                head_ref_name: Some("feature/current".to_string()),
                head_repository_owner: None,
                body: None,
                url: None,
                merge_commit: None,
            },
        ];
        let picked = select_preferred_pr(prs).expect("selected pr");
        assert_eq!(picked.number, 6693);
        assert_eq!(picked.state, "OPEN");
    }
}
