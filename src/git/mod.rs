use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow};

#[derive(Debug, Clone)]
pub struct Git {
    root: PathBuf,
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
        self.run(["branch", name, parent])
    }

    pub fn checkout_branch(&self, branch: &str) -> Result<()> {
        self.run(["checkout", branch])
    }

    pub fn delete_local_branch(&self, branch: &str) -> Result<()> {
        self.run(["branch", "-D", branch])
    }

    pub fn push_branch(&self, remote: &str, branch: &str) -> Result<()> {
        self.run(["push", "--set-upstream", remote, branch])
    }

    pub fn push_branch_force_with_lease(&self, remote: &str, branch: &str) -> Result<()> {
        self.run([
            "push",
            "--force-with-lease",
            "--set-upstream",
            remote,
            branch,
        ])
    }

    pub fn head_sha(&self, branch: &str) -> Result<String> {
        self.capture(["rev-parse", branch])
            .map(|s| s.trim().to_string())
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
        let output = Command::new("git")
            .current_dir(&self.root)
            .args(["remote", "get-url", remote])
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

    pub fn remote_for_branch(&self, branch: &str) -> Result<Option<String>> {
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

        Ok(Some("origin".to_string()))
    }

    pub fn base_remote_for_stack(&self, base_branch: &str) -> Result<String> {
        Ok(self
            .remote_for_branch(base_branch)?
            .unwrap_or_else(|| "origin".to_string()))
    }

    pub fn supports_replay(&self) -> bool {
        Command::new("git")
            .current_dir(&self.root)
            .args(["help", "-a"])
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).contains("replay"))
            .unwrap_or(false)
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
        let revision_range = format!("{old_base}..{branch}");
        let output = Command::new("git")
            .current_dir(&self.root)
            .args(["replay", "--onto", new_base, &revision_range])
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
        self.run(["rebase", "--onto", new_base, old_base, branch])
    }

    pub fn merge_base(&self, branch: &str, onto: &str) -> Result<String> {
        self.capture(["merge-base", branch, onto])
            .map(|s| s.trim().to_string())
    }

    pub fn is_ancestor(&self, ancestor: &str, branch: &str) -> Result<bool> {
        let status = Command::new("git")
            .current_dir(&self.root)
            .args(["merge-base", "--is-ancestor", ancestor, branch])
            .status()
            .with_context(|| format!("failed to compare ancestry {ancestor} -> {branch}"))?;
        Ok(status.success())
    }

    pub fn commit_distance(&self, base: &str, head: &str) -> Result<u32> {
        let out = self.capture(["rev-list", "--count", &format!("{base}..{head}")])?;
        let count = out
            .trim()
            .parse::<u32>()
            .with_context(|| format!("invalid commit distance output for {base}..{head}"))?;
        Ok(count)
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
