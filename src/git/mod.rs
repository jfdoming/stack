use std::path::PathBuf;
use std::process::Command;

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

    pub fn fetch_origin(&self) -> Result<()> {
        self.run(["fetch", "origin"])
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

    pub fn supports_replay(&self) -> bool {
        Command::new("git")
            .current_dir(&self.root)
            .args(["help", "-a"])
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).contains("replay"))
            .unwrap_or(false)
    }

    pub fn replay_onto(&self, branch: &str, old_base: &str, new_base: &str) -> Result<()> {
        self.run(["replay", "--onto", new_base, old_base, branch])
    }

    pub fn rebase_onto(&self, branch: &str, old_base: &str, new_base: &str) -> Result<()> {
        self.run(["rebase", "--onto", new_base, old_base, branch])
    }

    pub fn merge_base(&self, branch: &str, onto: &str) -> Result<String> {
        self.capture(["merge-base", branch, onto])
            .map(|s| s.trim().to_string())
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
