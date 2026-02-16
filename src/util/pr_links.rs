use anyhow::{Result, anyhow};

use crate::git::Git;
use crate::util::url::github_owner_from_web_url;

#[derive(Debug, Clone)]
pub struct PrLinkTarget {
    pub base_url: String,
    pub head_ref: String,
}

pub fn determine_pr_link_target(git: &Git, base: &str, head: &str) -> Result<PrLinkTarget> {
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
        .remote_for_branch(head)?
        .or_else(|| git.remote_for_branch(base).ok().flatten())
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

    Ok(PrLinkTarget { base_url, head_ref })
}
