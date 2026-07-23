use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow};

#[derive(Debug, Clone)]
pub struct Git {
    root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct RemoteInfo {
    pub name: String,
    pub fetch_url: Option<String>,
    pub push_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StashHandle {
    pub reference: String,
}

impl Git {
    pub fn discover() -> Result<Self> {
        let output = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .context("failed to run git rev-parse --show-toplevel")?;
        if !output.status.success() {
            return Err(anyhow!("not inside a git repository"));
        }
        let root = String::from_utf8(output.stdout)?.trim().to_string();
        Ok(Self {
            root: PathBuf::from(root),
        })
    }

    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    pub fn git_dir(&self) -> Result<PathBuf> {
        let out = self.capture(["rev-parse", "--git-dir"])?;
        let path = PathBuf::from(out.trim());
        if path.is_absolute() {
            Ok(path)
        } else {
            Ok(self.root.join(path))
        }
    }

    pub fn current_branch(&self) -> Result<String> {
        self.capture(["branch", "--show-current"])
            .map(|s| s.trim().to_string())
    }

    pub fn local_branches(&self) -> Result<Vec<String>> {
        let out = self.capture(["for-each-ref", "--format=%(refname:short)", "refs/heads"])?;
        Ok(out
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect())
    }

    pub fn branch_exists(&self, name: &str) -> Result<bool> {
        let status = Command::new("git")
            .current_dir(&self.root)
            .args([
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{name}"),
            ])
            .status()
            .with_context(|| format!("failed to verify branch {name}"))?;
        Ok(status.success())
    }

    pub fn create_branch_from(&self, name: &str, parent: &str) -> Result<()> {
        self.run(["branch", "--", name, parent])
    }

    pub fn checkout_branch(&self, branch: &str) -> Result<()> {
        self.run(["switch", "--", branch])
    }

    pub fn delete_local_branch(&self, branch: &str) -> Result<()> {
        self.run(["branch", "-D", "--", branch])
    }

    pub fn rename_local_branch(&self, old: &str, new: &str) -> Result<()> {
        self.run(["branch", "-m", "--", old, new])
    }

    pub fn push_branch(&self, remote: &str, branch: &str) -> Result<()> {
        let branch_ref = local_branch_ref(branch);
        let refspec = format!("{branch_ref}:{branch_ref}");
        self.run(["push", "--set-upstream", "--", remote, &refspec])
    }

    pub fn push_new_branch(&self, remote: &str, branch: &str) -> Result<()> {
        let branch_ref = local_branch_ref(branch);
        let lease = format!("--force-with-lease={branch_ref}:");
        let refspec = format!("{branch_ref}:{branch_ref}");
        self.run(["push", "--set-upstream", &lease, "--", remote, &refspec])
    }

    pub fn push_branch_force_with_lease(&self, remote: &str, branch: &str) -> Result<()> {
        let branch_ref = local_branch_ref(branch);
        let refspec = format!("{branch_ref}:{branch_ref}");
        self.run([
            "push",
            "--force-with-lease",
            "--set-upstream",
            "--",
            remote,
            &refspec,
        ])
    }

    pub fn delete_remote_branch(&self, remote: &str, branch: &str) -> Result<()> {
        let refspec = format!(":{}", local_branch_ref(branch));
        self.run(["push", "--", remote, &refspec])
    }

    pub fn remote_branch_exists(&self, remote: &str, branch: &str) -> Result<bool> {
        let remote_url = Command::new("git")
            .current_dir(&self.root)
            .args(["remote", "get-url", "--push", "--", remote])
            .output()
            .with_context(|| format!("failed to resolve push URL for remote {remote}"))?;
        if !remote_url.status.success() {
            return Err(anyhow!(
                "could not resolve push URL for remote '{}'",
                sanitize_terminal_text(remote)
            ));
        }
        let remote_url = String::from_utf8(remote_url.stdout)?.trim().to_string();
        if remote_url.is_empty() {
            return Err(anyhow!(
                "push URL for remote '{}' is empty",
                sanitize_terminal_text(remote)
            ));
        }

        let branch_ref = local_branch_ref(branch);
        let output = Command::new("git")
            .current_dir(&self.root)
            .args([
                "ls-remote",
                "--exit-code",
                "--heads",
                "--",
                &remote_url,
                &branch_ref,
            ])
            .output()
            .with_context(|| {
                format!(
                    "failed to inspect destination branch on remote '{}'",
                    sanitize_terminal_text(remote)
                )
            })?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(2) => Ok(false),
            _ => Err(anyhow!(
                "could not inspect destination branch on remote '{}'",
                sanitize_terminal_text(remote)
            )),
        }
    }

    pub fn head_sha(&self, branch: &str) -> Result<String> {
        let revision = format!("{}^{{commit}}", local_branch_ref(branch));
        self.capture(["rev-parse", "--verify", "--end-of-options", &revision])
            .map(|s| s.trim().to_string())
    }

    pub fn resolve_commit(&self, rev: &str) -> Result<String> {
        let rev = self.unambiguous_revision(rev)?;
        let revision = format!("{rev}^{{commit}}");
        self.capture(["rev-parse", "--verify", "--end-of-options", &revision])
            .map(|s| s.trim().to_string())
    }

    pub fn rev_list_reverse(&self, base: &str, head: &str) -> Result<Vec<String>> {
        let range = format!(
            "{}..{}",
            self.resolve_commit(base)?,
            self.resolve_commit(head)?
        );
        let out = self.capture(["rev-list", "--reverse", "--end-of-options", &range, "--"])?;
        Ok(out
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect())
    }

    pub fn has_merge_commits(&self, base: &str, head: &str) -> Result<bool> {
        let range = format!(
            "{}..{}",
            self.resolve_commit(base)?,
            self.resolve_commit(head)?
        );
        let out = self.capture(["rev-list", "--merges", "--end-of-options", &range, "--"])?;
        Ok(out.lines().any(|line| !line.trim().is_empty()))
    }

    pub fn commit_oneline(&self, rev: &str) -> Result<String> {
        let commit = self.resolve_commit(rev)?;
        self.capture(["show", "-s", "--format=%h %s", "--end-of-options", &commit])
            .map(|s| s.trim().to_string())
    }

    pub fn is_valid_branch_name(&self, name: &str) -> Result<bool> {
        let output = Command::new("git")
            .current_dir(&self.root)
            .args(["check-ref-format", "--branch", name])
            .output()
            .with_context(|| format!("failed to validate branch name {name}"))?;
        if !output.status.success() {
            return Ok(false);
        }
        Ok(String::from_utf8(output.stdout)?.trim() == name)
    }

    pub fn is_worktree_dirty(&self) -> Result<bool> {
        let status = Command::new("git")
            .current_dir(&self.root)
            .args(["diff", "--quiet", "--ignore-submodules", "HEAD", "--"])
            .status()
            .context("failed to check worktree state")?;
        Ok(!status.success())
    }

    pub fn stash_push(&self, reason: &str) -> Result<Option<StashHandle>> {
        let status = Command::new("git")
            .current_dir(&self.root)
            .args(["stash", "push", "-u", "-m", reason])
            .output()
            .context("failed to run git stash push")?;
        if !status.status.success() {
            return Err(anyhow!(
                "git stash push failed: {}",
                String::from_utf8_lossy(&status.stderr)
            ));
        }
        let stdout = String::from_utf8(status.stdout)?;
        if stdout.contains("No local changes to save") {
            return Ok(None);
        }
        Ok(Some(StashHandle {
            reference: "stash@{0}".to_string(),
        }))
    }

    pub fn stash_pop(&self, stash: &StashHandle) -> Result<()> {
        self.run(["stash", "pop", &stash.reference])
    }

    pub fn fetch_remote(&self, remote: &str) -> Result<()> {
        if !self.has_remote(remote)? {
            eprintln!("warning: no '{remote}' remote configured; skipping fetch");
            return Ok(());
        }
        self.run(["fetch", remote])
    }

    pub fn preferred_sync_remote(&self, base_remote: &str) -> Result<String> {
        if self.has_remote("upstream")? {
            return Ok("upstream".to_string());
        }
        Ok(base_remote.to_string())
    }

    pub fn default_base_branch(&self) -> Result<String> {
        let output = Command::new("git")
            .current_dir(&self.root)
            .args(["symbolic-ref", "refs/remotes/origin/HEAD"])
            .output()
            .context("failed to read origin/HEAD")?;

        if output.status.success() {
            let val = String::from_utf8(output.stdout)?.trim().to_string();
            if let Some(branch) = val.strip_prefix("refs/remotes/origin/") {
                return Ok(branch.to_string());
            }
        }
        Ok("main".to_string())
    }

    pub fn remote_web_url(&self, remote: &str) -> Result<Option<String>> {
        self.remote_url(remote, false)
    }

    pub fn remote_push_web_url(&self, remote: &str) -> Result<Option<String>> {
        self.remote_url(remote, true)
    }

    fn remote_url(&self, remote: &str, push: bool) -> Result<Option<String>> {
        let mut command = Command::new("git");
        command.current_dir(&self.root).args(["remote", "get-url"]);
        if push {
            command.arg("--push");
        }
        let output = command
            .arg(remote)
            .output()
            .with_context(|| format!("failed to read {remote} remote URL"))?;
        if !output.status.success() {
            return Ok(None);
        }
        let raw = String::from_utf8(output.stdout)?.trim().to_string();
        if raw.is_empty() {
            return Ok(None);
        }
        Ok(parse_remote_to_web_url(&raw))
    }

    pub fn remote_infos(&self) -> Result<Vec<RemoteInfo>> {
        let output = Command::new("git")
            .current_dir(&self.root)
            .args(["remote"])
            .output()
            .context("failed to list git remotes")?;
        if !output.status.success() {
            return Ok(Vec::new());
        }
        let names = String::from_utf8(output.stdout)?;
        names
            .lines()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(|name| {
                Ok(RemoteInfo {
                    name: name.to_string(),
                    fetch_url: self.remote_web_url(name)?,
                    push_url: self.remote_push_web_url(name)?,
                })
            })
            .collect()
    }

    pub fn remote_for_branch(&self, branch: &str) -> Result<Option<String>> {
        if let Some(remote) = self.configured_remote_for_branch(branch)? {
            return Ok(Some(remote));
        }
        Ok(Some("origin".to_string()))
    }

    pub fn configured_remote_for_branch(&self, branch: &str) -> Result<Option<String>> {
        let config_key = format!("branch.{branch}.remote");
        let output = Command::new("git")
            .current_dir(&self.root)
            .args(["config", "--get", &config_key])
            .output()
            .with_context(|| format!("failed to read {config_key}"))?;

        if output.status.success() {
            let remote = String::from_utf8(output.stdout)?.trim().to_string();
            if !remote.is_empty() {
                return Ok(Some(remote));
            }
        }

        let upstream = self
            .capture([
                "for-each-ref",
                "--format=%(upstream:short)",
                &format!("refs/heads/{branch}"),
            ])
            .unwrap_or_default();
        let upstream = upstream.trim();
        if let Some((remote, _)) = upstream.split_once('/')
            && !remote.is_empty()
        {
            return Ok(Some(remote.to_string()));
        }

        Ok(None)
    }

    pub fn preferred_remote_for_branch(
        &self,
        branch: &str,
        fallback_branch: &str,
    ) -> Result<String> {
        Ok(self
            .configured_remote_for_branch(branch)?
            .or(self.configured_remote_for_branch(fallback_branch)?)
            .unwrap_or_else(|| "origin".to_string()))
    }

    pub fn base_remote_for_stack(&self, base_branch: &str) -> Result<String> {
        Ok(self
            .remote_for_branch(base_branch)?
            .unwrap_or_else(|| "origin".to_string()))
    }

    pub fn branch_upstream(&self, branch: &str) -> Result<Option<String>> {
        let output = Command::new("git")
            .current_dir(&self.root)
            .args([
                "for-each-ref",
                "--format=%(upstream:short)",
                &format!("refs/heads/{branch}"),
            ])
            .output()
            .with_context(|| format!("failed to resolve upstream for branch {branch}"))?;
        if !output.status.success() {
            return Ok(None);
        }
        let upstream = String::from_utf8(output.stdout)?.trim().to_string();
        if upstream.is_empty() {
            return Ok(None);
        }
        Ok(Some(upstream))
    }

    pub fn supports_replay(&self) -> bool {
        Command::new("git")
            .current_dir(&self.root)
            .args(["help", "-a"])
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).contains("replay"))
            .unwrap_or(false)
    }

    pub fn ref_exists(&self, name: &str) -> Result<bool> {
        let name = self.unambiguous_revision(name)?;
        let revision = format!("{name}^{{commit}}");
        let output = Command::new("git")
            .current_dir(&self.root)
            .args([
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                &revision,
            ])
            .output()
            .with_context(|| format!("failed to verify ref {name}"))?;
        Ok(output.status.success())
    }

    pub fn fast_forward_branch(&self, branch: &str, onto: &str) -> Result<()> {
        let onto = self.resolve_commit(onto)?;
        self.checkout_branch(branch)?;
        self.run(["merge", "--ff-only", "--", &onto])
    }

    fn has_remote(&self, name: &str) -> Result<bool> {
        let output = Command::new("git")
            .current_dir(&self.root)
            .args(["remote"])
            .output()
            .context("failed to list git remotes")?;
        if !output.status.success() {
            return Ok(false);
        }
        let remotes = String::from_utf8(output.stdout)?;
        Ok(remotes.lines().any(|line| line.trim() == name))
    }

    pub fn replay_onto(&self, branch: &str, old_base: &str, new_base: &str) -> Result<()> {
        let old_base = self.resolve_commit(old_base)?;
        let new_base = self.resolve_commit(new_base)?;
        let revision_range = format!("{old_base}..{}", local_branch_ref(branch));
        let output = Command::new("git")
            .current_dir(&self.root)
            .args(["replay", "--onto", &new_base, "--", &revision_range])
            .output()
            .with_context(|| {
                format!(
                    "failed to run git [\"replay\", \"--onto\", \"{new_base}\", \"{revision_range}\"]"
                )
            })?;
        if !output.status.success() {
            return Err(anyhow!(
                "git command failed [\"replay\", \"--onto\", \"{}\", \"{}\"]: {}",
                new_base,
                revision_range,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        if !output.stdout.is_empty() {
            let mut apply = Command::new("git")
                .current_dir(&self.root)
                .args(["update-ref", "--stdin"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .context("failed to run git update-ref --stdin")?;
            if let Some(stdin) = apply.stdin.as_mut() {
                stdin
                    .write_all(&output.stdout)
                    .context("failed to write git replay ref updates")?;
            }
            let apply_output = apply
                .wait_with_output()
                .context("failed to apply git replay ref updates")?;
            if !apply_output.status.success() {
                return Err(anyhow!(
                    "git command failed [\"update-ref\", \"--stdin\"]: {}",
                    String::from_utf8_lossy(&apply_output.stderr)
                ));
            }
        }
        Ok(())
    }

    pub fn rebase_onto(&self, branch: &str, old_base: &str, new_base: &str) -> Result<()> {
        let old_base = self.resolve_commit(old_base)?;
        let new_base = self.resolve_commit(new_base)?;
        self.checkout_branch(branch)?;
        self.run(["rebase", "--onto", &new_base, "--", &old_base])
    }

    pub fn merge_base(&self, branch: &str, onto: &str) -> Result<String> {
        let branch = self.resolve_commit(branch)?;
        let onto = self.resolve_commit(onto)?;
        self.capture(["merge-base", "--", &branch, &onto])
            .map(|s| s.trim().to_string())
    }

    pub fn is_ancestor(&self, ancestor: &str, branch: &str) -> Result<bool> {
        let ancestor = self.resolve_commit(ancestor)?;
        let branch = self.resolve_commit(branch)?;
        let status = Command::new("git")
            .current_dir(&self.root)
            .args(["merge-base", "--is-ancestor", "--", &ancestor, &branch])
            .status()
            .with_context(|| format!("failed to compare ancestry {ancestor} -> {branch}"))?;
        Ok(status.success())
    }

    pub fn commit_distance(&self, base: &str, head: &str) -> Result<u32> {
        let range = format!(
            "{}..{}",
            self.resolve_commit(base)?,
            self.resolve_commit(head)?
        );
        let out = self.capture(["rev-list", "--count", "--end-of-options", &range, "--"])?;
        let count = out
            .trim()
            .parse::<u32>()
            .with_context(|| format!("invalid commit distance output for {base}..{head}"))?;
        Ok(count)
    }

    fn unambiguous_revision(&self, revision: &str) -> Result<String> {
        if self.branch_exists(revision)? {
            Ok(local_branch_ref(revision))
        } else {
            Ok(revision.to_string())
        }
    }

    pub fn capture<const N: usize>(&self, args: [&str; N]) -> Result<String> {
        let output = Command::new("git")
            .current_dir(&self.root)
            .args(args)
            .output()
            .with_context(|| format!("failed to run git {:?}", args))?;
        if !output.status.success() {
            return Err(anyhow!(
                "git command failed {:?}: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(String::from_utf8(output.stdout)?)
    }

    pub fn run<const N: usize>(&self, args: [&str; N]) -> Result<()> {
        let output = Command::new("git")
            .current_dir(&self.root)
            .args(args)
            .output()
            .with_context(|| format!("failed to run git {:?}", args))?;
        if !output.status.success() {
            return Err(anyhow!(
                "git command failed {:?}: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(())
    }
}

fn local_branch_ref(branch: &str) -> String {
    format!("refs/heads/{branch}")
}

fn parse_remote_to_web_url(raw: &str) -> Option<String> {
    if let Some(rest) = raw.strip_prefix("git@")
        && let Some((host, repo)) = rest.split_once(':')
    {
        return Some(sanitize_terminal_text(&format!(
            "https://{}/{}",
            host.trim_end_matches('/'),
            repo.trim_end_matches(".git")
        )));
    }

    if let Some(rest) = raw.strip_prefix("ssh://git@")
        && let Some((host, repo)) = rest.split_once('/')
    {
        return Some(sanitize_terminal_text(&format!(
            "https://{}/{}",
            host.trim_end_matches('/'),
            repo.trim_end_matches(".git")
        )));
    }

    if raw.starts_with("https://") || raw.starts_with("http://") {
        return Some(sanitize_terminal_text(
            raw.trim_end_matches(".git").trim_end_matches('/').trim(),
        ));
    }

    None
}

fn sanitize_terminal_text(value: &str) -> String {
    value.chars().filter(|c| !c.is_control()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_remote_to_web_url_strips_control_characters() {
        let parsed = parse_remote_to_web_url("https://github.com/acme/repo\u{1b}[31m")
            .expect("url should parse");
        assert_eq!(parsed, "https://github.com/acme/repo[31m");
    }

    #[test]
    fn parse_remote_to_web_url_normalizes_git_ssh_remote() {
        let parsed =
            parse_remote_to_web_url("git@github.com:acme/repo.git").expect("url should parse");
        assert_eq!(parsed, "https://github.com/acme/repo");
    }
}
