use std::collections::{HashMap, HashSet};
use std::process::Command;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;

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
    pub head_ref_oid: Option<String>,
    pub merge_commit_oid: Option<String>,
    pub base_ref_name: Option<String>,
    pub body: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RepositoryInfo {
    pub name_with_owner: String,
    pub viewer_permission: Option<String>,
    pub parent_name_with_owner: Option<String>,
    pub parent_viewer_permission: Option<String>,
}

pub trait Provider {
    fn repository_info(&self, _repo_slug: &str) -> Result<Option<RepositoryInfo>> {
        Ok(None)
    }
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
    fn update_pr_base(&self, pr_number: i64, base: &str) -> Result<()>;
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
        for remote in self.git.remote_infos()? {
            for url in [remote.fetch_url.as_deref(), remote.push_url.as_deref()]
                .into_iter()
                .flatten()
            {
                if let Some(slug) = github_repo_slug_from_web_url(url)
                    && seen.insert(slug.clone())
                {
                    out.push(slug);
                }
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

        for remote in self.git.remote_infos()? {
            for url in [remote.fetch_url.as_deref(), remote.push_url.as_deref()]
                .into_iter()
                .flatten()
            {
                if let Some(slug) = github_repo_slug_from_web_url(url)
                    && seen.insert(slug.clone())
                {
                    out.push(slug);
                }
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

    fn default_repo_scope_from_gh(&self) -> Result<Option<String>> {
        let args = ["repo", "view", "--json", "nameWithOwner"];
        let Some(raw) = self.run_gh_optional(&args)? else {
            return Ok(None);
        };
        if raw.trim().is_empty() {
            return Ok(None);
        }
        let cleaned = clean_gh_json_output(&raw);
        let Ok(view) = serde_json::from_str::<GhRepoView>(&cleaned) else {
            return Ok(None);
        };
        if view.name_with_owner.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some(view.name_with_owner))
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
    #[serde(rename = "headRefOid")]
    head_ref_oid: Option<String>,
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

#[derive(Debug, Deserialize)]
struct GhRepoView {
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
}

#[derive(Debug, Deserialize)]
struct GhRepositoryInfo {
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
    #[serde(rename = "viewerPermission")]
    viewer_permission: Option<String>,
    parent: Option<GhRepositoryParent>,
}

#[derive(Debug, Deserialize)]
struct GhRepositoryParent {
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
    #[serde(rename = "viewerPermission")]
    viewer_permission: Option<String>,
}

impl Provider for GithubProvider {
    fn repository_info(&self, repo_slug: &str) -> Result<Option<RepositoryInfo>> {
        let Some((owner, name)) = parse_repo_scope(repo_slug) else {
            return Ok(None);
        };
        let query = "query($owner:String!, $name:String!) { repository(owner:$owner, name:$name) { nameWithOwner viewerPermission parent { nameWithOwner viewerPermission } } }";
        let args = [
            "api",
            "graphql",
            "-f",
            &format!("query={query}"),
            "-F",
            &format!("owner={owner}"),
            "-F",
            &format!("name={name}"),
        ];
        let Some(raw) = self.run_gh_optional(&args)? else {
            return Ok(None);
        };
        parse_repository_info_response(&raw)
    }

    fn resolve_prs_by_head(
        &self,
        branches: &[(&str, Option<i64>)],
    ) -> Result<HashMap<String, PrInfo>> {
        let mut out = HashMap::new();
        if branches.is_empty() {
            return Ok(out);
        }

        let mut by_head: HashMap<String, Vec<GhPr>> = HashMap::new();
        let mut repo_scopes = self.repo_scope_candidates_for_branches(branches)?;
        if repo_scopes.is_empty() {
            if let Some(scope) = self.default_repo_scope_from_gh()? {
                repo_scopes.push(scope);
            } else {
                return Ok(out);
            }
        }

        let mut unique_heads: Vec<String> = branches
            .iter()
            .map(|(name, _)| (*name).to_string())
            .collect();
        unique_heads.sort();
        unique_heads.dedup();

        for scope in repo_scopes {
            let Some((owner, repo)) = parse_repo_scope(&scope) else {
                continue;
            };

            for chunk in unique_heads.chunks(HEAD_QUERY_CHUNK_SIZE) {
                let (query, query_fields) = build_head_lookup_query(chunk);
                let mut args = vec![
                    "api".to_string(),
                    "graphql".to_string(),
                    "-f".to_string(),
                    format!("query={query}"),
                    "-F".to_string(),
                    format!("owner={owner}"),
                    "-F".to_string(),
                    format!("name={repo}"),
                ];
                args.extend(query_fields);
                let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
                let Some(raw) = self.run_gh_optional(&arg_refs)? else {
                    continue;
                };
                if raw.trim().is_empty() {
                    continue;
                }

                let by_alias = match parse_graphql_head_lookup_result(&raw) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if by_alias.is_empty() {
                    continue;
                }
                for prs in by_alias.into_values() {
                    for pr in prs {
                        if let Some(head) = pr.head_ref_name.as_deref()
                            && !head.is_empty()
                        {
                            by_head.entry(head.to_string()).or_default().push(pr);
                        }
                    }
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
            let scopes = build_scope_options(self.repo_scope_candidates_for_branch(branch)?);
            for scope in scopes {
                let mut args = vec![
                    "pr".to_string(),
                    "view".to_string(),
                    num.to_string(),
                    "--json".to_string(),
                    "number,state,headRefOid,mergeCommit,baseRefName,url,body".to_string(),
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
                let trimmed = out.trim();
                if trimmed.starts_with('[') {
                    let prs = self.parse_gh_pr_list(&out, context)?;
                    if let Some(pr) = select_preferred_pr(prs) {
                        return Ok(Some(convert_pr(pr)));
                    }
                    continue;
                }
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

        let scopes = build_scope_options(self.repo_scope_candidates_for_branch(branch)?);
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
                    "number,state,headRefOid,mergeCommit,baseRefName,url,body".to_string(),
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

    fn update_pr_base(&self, pr_number: i64, base: &str) -> Result<()> {
        let num = pr_number.to_string();
        let args = ["pr", "edit", &num, "--base", base];
        let _ = self.run_gh_required(&args)?;
        Ok(())
    }
}

fn build_scope_options(scopes: Vec<String>) -> Vec<Option<String>> {
    if scopes.is_empty() {
        return vec![None];
    }
    scopes.into_iter().map(Some).collect()
}

const HEAD_QUERY_CHUNK_SIZE: usize = 20;

fn parse_repo_scope(scope: &str) -> Option<(&str, &str)> {
    let (owner, repo) = scope.split_once('/')?;
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some((owner, repo))
}

fn build_head_lookup_query(heads: &[String]) -> (String, Vec<String>) {
    let mut query = String::from("query($owner:String!, $name:String!");
    for idx in 0..heads.len() {
        query.push_str(&format!(", $h{idx}:String!"));
    }
    query.push_str(") { repository(owner:$owner, name:$name) {");
    for idx in 0..heads.len() {
        query.push_str(&format!(
            " h{idx}: pullRequests(first:20, states:[OPEN,MERGED,CLOSED], headRefName:$h{idx}, orderBy:{{field:UPDATED_AT, direction:DESC}}) {{ nodes {{ number state headRefOid mergeCommit {{ oid }} baseRefName headRefName headRepositoryOwner {{ login }} url body }} }}"
        ));
    }
    query.push_str(" } }");

    let mut fields = Vec::with_capacity(heads.len() * 2);
    for (idx, head) in heads.iter().enumerate() {
        fields.push("-f".to_string());
        fields.push(format!("h{idx}={head}"));
    }
    (query, fields)
}

fn parse_graphql_head_lookup_result(raw: &str) -> Result<HashMap<String, Vec<GhPr>>> {
    let cleaned = clean_gh_json_output(raw);
    let parsed: Value = serde_json::from_str(&cleaned)?;
    let mut out = HashMap::new();

    let Some(repo_obj) = parsed
        .get("data")
        .and_then(|v| v.get("repository"))
        .and_then(Value::as_object)
    else {
        return Ok(out);
    };

    for (alias, payload) in repo_obj {
        let Some(nodes) = payload.get("nodes") else {
            continue;
        };
        let prs: Vec<GhPr> = serde_json::from_value(nodes.clone())?;
        out.insert(alias.clone(), prs);
    }

    Ok(out)
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
        head_ref_oid: pr.head_ref_oid,
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
                head_ref_oid: pr.head_ref_oid.clone(),
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

fn parse_repository_info_response(raw: &str) -> Result<Option<RepositoryInfo>> {
    let cleaned = clean_gh_json_output(raw);
    let parsed: Value = serde_json::from_str(&cleaned)?;
    let Some(repository) = parsed.get("data").and_then(|value| value.get("repository")) else {
        return Ok(None);
    };
    if repository.is_null() {
        return Ok(None);
    }
    let info: GhRepositoryInfo = serde_json::from_value(repository.clone())?;
    Ok(Some(RepositoryInfo {
        name_with_owner: info.name_with_owner,
        viewer_permission: info.viewer_permission,
        parent_name_with_owner: info
            .parent
            .as_ref()
            .map(|parent| parent.name_with_owner.clone()),
        parent_viewer_permission: info.parent.and_then(|parent| parent.viewer_permission),
    }))
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
                head_ref_oid: None,
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
                head_ref_oid: None,
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

    #[test]
    fn build_scope_options_omits_default_when_repo_scopes_exist() {
        let scopes = build_scope_options(vec!["acme/repo".to_string()]);
        assert_eq!(scopes, vec![Some("acme/repo".to_string())]);
    }

    #[test]
    fn parse_repo_scope_extracts_owner_and_repo() {
        assert_eq!(parse_repo_scope("acme/repo"), Some(("acme", "repo")));
        assert_eq!(parse_repo_scope("acme"), None);
    }

    #[test]
    fn build_head_lookup_query_includes_aliases_and_head_vars() {
        let heads = vec!["feat/a".to_string(), "feat/b".to_string()];
        let (query, fields) = build_head_lookup_query(&heads);
        assert!(query.contains("h0: pullRequests"));
        assert!(query.contains("h1: pullRequests"));
        assert!(query.contains("$h0:String!"));
        assert!(query.contains("$h1:String!"));
        assert!(query.contains("headRefOid"));
        assert_eq!(fields, vec!["-f", "h0=feat/a", "-f", "h1=feat/b"]);
    }

    #[test]
    fn build_head_lookup_query_uses_raw_fields_for_file_like_heads() {
        let heads = vec!["@branch-name".to_string()];
        let (_, fields) = build_head_lookup_query(&heads);
        assert_eq!(fields, vec!["-f", "h0=@branch-name"]);
    }

    #[test]
    fn parse_graphql_head_lookup_result_extracts_pr_nodes() {
        let raw = r#"{
          "data": {
            "repository": {
              "h0": {
                "nodes": [
                  {
                    "number": 10,
                    "state": "OPEN",
                    "mergeCommit": null,
                    "baseRefName": "main",
                    "headRefName": "feat/a",
                    "headRefOid": "abc123",
                    "headRepositoryOwner": {"login": "acme"},
                    "url": "https://example.com/pull/10",
                    "body": "test"
                  }
                ]
              }
            }
          }
        }"#;
        let parsed = parse_graphql_head_lookup_result(raw).expect("parsed graphql result");
        let h0 = parsed.get("h0").expect("h0 alias");
        assert_eq!(h0.len(), 1);
        assert_eq!(h0[0].number, 10);
        assert_eq!(h0[0].head_ref_name.as_deref(), Some("feat/a"));
        assert_eq!(h0[0].head_ref_oid.as_deref(), Some("abc123"));
    }

    #[test]
    fn parse_repository_info_preserves_each_permission_level() {
        for permission in ["READ", "TRIAGE", "WRITE", "MAINTAIN", "ADMIN"] {
            let raw = format!(
                r#"{{"data":{{"repository":{{"nameWithOwner":"acme/repo","viewerPermission":"{permission}","parent":null}}}}}}"#
            );
            let info = parse_repository_info_response(&raw)
                .expect("parse repository response")
                .expect("repository metadata");
            assert_eq!(info.name_with_owner, "acme/repo");
            assert_eq!(info.viewer_permission.as_deref(), Some(permission));
        }
    }

    #[test]
    fn parse_repository_info_extracts_fork_parent_topology() {
        let raw = r#"{
          "data": {
            "repository": {
              "nameWithOwner": "alice/repo",
              "viewerPermission": "ADMIN",
              "parent": {
                "nameWithOwner": "acme/repo",
                "viewerPermission": "WRITE"
              }
            }
          }
        }"#;
        let info = parse_repository_info_response(raw)
            .expect("parse repository response")
            .expect("repository metadata");
        assert_eq!(info.name_with_owner, "alice/repo");
        assert_eq!(info.parent_name_with_owner.as_deref(), Some("acme/repo"));
        assert_eq!(info.parent_viewer_permission.as_deref(), Some("WRITE"));
    }
}
