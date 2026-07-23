use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, anyhow};
use url::Url;

use crate::db::BaseBranchSource;

static NEXT_AUTO_STASH_ID: AtomicU64 = AtomicU64::new(0);
static NEXT_RESTACK_ID: AtomicU64 = AtomicU64::new(0);

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

#[derive(Debug, Clone)]
pub struct BaseBranchCandidate {
    pub name: String,
    pub source: BaseBranchSource,
    pub remote: Option<String>,
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
        self.resolve_git_path(["rev-parse", "--path-format=absolute", "--git-dir"])
    }

    pub fn common_git_dir(&self) -> Result<PathBuf> {
        self.resolve_git_path(["rev-parse", "--path-format=absolute", "--git-common-dir"])
    }

    pub fn stack_db_path(&self) -> Result<PathBuf> {
        Ok(self.common_git_dir()?.join("stack.db"))
    }

    fn resolve_git_path<const N: usize>(&self, args: [&str; N]) -> Result<PathBuf> {
        let out = self.capture(args)?;
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

    pub fn create_branch_from_branch(&self, name: &str, parent: &str) -> Result<()> {
        let parent_ref = local_branch_ref(parent);
        self.run(["branch", "--", name, &parent_ref])
    }

    pub fn checkout_branch(&self, branch: &str) -> Result<()> {
        self.run(["switch", "--", branch])
    }

    pub fn delete_local_branch(&self, branch: &str) -> Result<()> {
        self.run(["branch", "-D", "--", branch])
    }

    pub fn delete_local_branch_if_unchanged(
        &self,
        branch: &str,
        expected_head: &str,
    ) -> Result<()> {
        let branch_ref = local_branch_ref(branch);
        if self.branch_checked_out_in_worktree(&branch_ref)? {
            return Err(anyhow!(
                "refusing to delete '{}': branch is checked out in a worktree",
                sanitize_terminal_text(branch)
            ));
        }

        let output = Command::new("git")
            .current_dir(&self.root)
            .args(["update-ref", "-d", &branch_ref, expected_head])
            .output()
            .with_context(|| format!("failed to conditionally delete branch {branch}"))?;
        if !output.status.success() {
            return Err(anyhow!(
                "refusing to delete '{}': branch changed before deletion: {}",
                sanitize_terminal_text(branch),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }

        if self.branch_checked_out_in_worktree(&branch_ref)? {
            let restore = Command::new("git")
                .current_dir(&self.root)
                .args(["update-ref", &branch_ref, expected_head, ""])
                .output()
                .with_context(|| format!("failed to restore checked-out branch {branch}"))?;
            if !restore.status.success() {
                return Err(anyhow!(
                    "branch '{}' became checked out during deletion and its ref could not be restored safely: {}",
                    sanitize_terminal_text(branch),
                    String::from_utf8_lossy(&restore.stderr).trim()
                ));
            }
            return Err(anyhow!(
                "refusing to delete '{}': branch became checked out in a worktree; its ref was restored",
                sanitize_terminal_text(branch)
            ));
        }

        let section = format!("branch.{branch}");
        let _ = Command::new("git")
            .current_dir(&self.root)
            .args(["config", "--local", "--remove-section", &section])
            .output();
        Ok(())
    }

    fn branch_checked_out_in_worktree(&self, branch_ref: &str) -> Result<bool> {
        let worktrees = Command::new("git")
            .current_dir(&self.root)
            .args(["worktree", "list", "--porcelain", "-z"])
            .output()
            .context("failed to inspect linked worktree checkouts")?;
        if !worktrees.status.success() {
            return Err(anyhow!(
                "git worktree list failed before deleting '{}': {}",
                sanitize_terminal_text(branch_ref),
                String::from_utf8_lossy(&worktrees.stderr)
            ));
        }
        let checked_out_field = format!("branch {branch_ref}");
        Ok(worktrees
            .stdout
            .split(|byte| *byte == 0)
            .any(|field| field == checked_out_field.as_bytes()))
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

    pub fn first_parent_commit_oids(&self, branch: &str) -> Result<Vec<String>> {
        let reference = local_branch_ref(branch);
        let out = self.capture(["rev-list", "--first-parent", &reference])?;
        Ok(out
            .lines()
            .map(str::trim)
            .filter(|oid| !oid.is_empty())
            .map(ToOwned::to_owned)
            .collect())
    }

    pub fn is_first_parent_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool> {
        if !self.object_id_exists(ancestor)? || !self.object_id_exists(descendant)? {
            return Ok(false);
        }
        let ancestor = self.resolve_object_id(ancestor)?;
        let descendant = self.resolve_object_id(descendant)?;
        let out = self.capture(["rev-list", "--first-parent", &descendant])?;
        Ok(out.lines().any(|oid| oid.trim() == ancestor))
    }

    pub fn resolve_commit(&self, rev: &str) -> Result<String> {
        let rev = self.unambiguous_revision(rev)?;
        let revision = format!("{rev}^{{commit}}");
        self.capture(["rev-parse", "--verify", "--end-of-options", &revision])
            .map(|s| s.trim().to_string())
    }

    pub fn resolve_object_id(&self, oid: &str) -> Result<String> {
        self.exact_object_id_commit(oid)?.ok_or_else(|| {
            anyhow!("full Git object ID does not resolve to that exact commit object")
        })
    }

    pub fn object_id_exists(&self, oid: &str) -> Result<bool> {
        Ok(self.exact_object_id_commit(oid)?.is_some())
    }

    fn exact_object_id_commit(&self, oid: &str) -> Result<Option<String>> {
        if !valid_object_id(oid) {
            return Err(anyhow!("invalid full Git object ID"));
        }
        let revision = format!("{oid}^{{commit}}");
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
            .context("failed to resolve full Git object ID")?;
        match output.status.code() {
            Some(0) => {
                let resolved = String::from_utf8(output.stdout)?.trim().to_string();
                Ok(resolved.eq_ignore_ascii_case(oid).then_some(resolved))
            }
            Some(1) => Ok(None),
            _ => Err(anyhow!(
                "could not resolve full Git object ID: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )),
        }
    }

    pub fn rev_list_reverse(&self, base: &str, head: &str) -> Result<Vec<String>> {
        let range = format!("{}..{}", self.head_sha(base)?, self.head_sha(head)?);
        let out = self.capture(["rev-list", "--reverse", "--end-of-options", &range, "--"])?;
        Ok(out
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect())
    }

    pub fn has_merge_commits(&self, base: &str, head: &str) -> Result<bool> {
        let range = format!("{}..{}", self.head_sha(base)?, self.head_sha(head)?);
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
        let output = Command::new("git")
            .current_dir(&self.root)
            .args(["status", "--porcelain=v1", "--untracked-files=normal", "--"])
            .output()
            .context("failed to check worktree state")?;
        if !output.status.success() {
            return Err(anyhow!(
                "git status failed while checking worktree state: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(!output.stdout.is_empty())
    }

    pub fn stash_push(&self, reason: &str) -> Result<Option<StashHandle>> {
        let marker = format!(
            "{reason} [stack-auto-stash:{}:{}]",
            std::process::id(),
            NEXT_AUTO_STASH_ID.fetch_add(1, Ordering::Relaxed)
        );
        let status = Command::new("git")
            .current_dir(&self.root)
            .args(["stash", "push", "-u", "-m", &marker])
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
        let entries = self.capture(["stash", "list", "--format=%H%x09%gs"])?;
        let reference = entries
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .find_map(|(oid, subject)| subject.ends_with(&marker).then(|| oid.to_string()))
            .ok_or_else(|| anyhow!("could not identify the auto-stash created for this sync"))?;
        Ok(Some(StashHandle { reference }))
    }

    pub fn stash_restore(&self, stash: &StashHandle) -> Result<()> {
        self.run(["stash", "apply", &stash.reference])
    }

    pub fn fetch_remote(&self, remote: &str) -> Result<()> {
        if !self.has_remote(remote)? {
            eprintln!("warning: no '{remote}' remote configured; skipping fetch");
            return Ok(());
        }
        self.run(["fetch", "--", remote])
    }

    pub fn preferred_sync_remote(&self, base_remote: &str) -> Result<String> {
        if self.has_remote("upstream")? {
            return Ok("upstream".to_string());
        }
        Ok(base_remote.to_string())
    }

    pub fn default_base_branch(&self) -> Result<BaseBranchCandidate> {
        self.default_base_branch_with_hint(None, None)
    }

    pub fn default_base_branch_with_hint(
        &self,
        preferred_branch: Option<&str>,
        preferred_remote: Option<&str>,
    ) -> Result<BaseBranchCandidate> {
        let local = self.local_branches()?;
        let local_candidate = if let Some(name) = ["main", "master", "trunk", "develop"]
            .into_iter()
            .find(|conventional| local.iter().any(|branch| branch == conventional))
        {
            BaseBranchCandidate {
                name: name.to_string(),
                source: BaseBranchSource::LocalConvention,
                remote: None,
            }
        } else {
            let current = self.current_branch()?;
            if !current.is_empty() {
                BaseBranchCandidate {
                    name: current,
                    source: BaseBranchSource::CurrentBranch,
                    remote: None,
                }
            } else if let Some(name) = local.first() {
                BaseBranchCandidate {
                    name: name.clone(),
                    source: BaseBranchSource::FirstLocal,
                    remote: None,
                }
            } else {
                BaseBranchCandidate {
                    name: "main".to_string(),
                    source: BaseBranchSource::Default,
                    remote: None,
                }
            }
        };

        if let Some(remote) = preferred_remote {
            return Ok(self
                .remote_head_base_candidate(remote, &local)?
                .unwrap_or(local_candidate));
        }
        if let Some(branch) = preferred_branch
            && let Some(remote) = self.configured_remote_for_branch(branch)?
            && let Some(candidate) = self.remote_head_base_candidate(&remote, &local)?
        {
            return Ok(candidate);
        }
        if let Some(remote) = self.configured_remote_for_branch(&local_candidate.name)?
            && let Some(candidate) = self.remote_head_base_candidate(&remote, &local)?
        {
            return Ok(candidate);
        }

        if let Some(origin_candidate) = self.remote_head_base_candidate("origin", &local)? {
            return Ok(origin_candidate);
        }

        let mut configured_remotes = std::collections::BTreeSet::new();
        for branch in &local {
            if let Some(remote) = self.configured_remote_for_branch(branch)?
                && remote != "."
                && remote != "origin"
            {
                configured_remotes.insert(remote);
            }
        }
        let mut configured_candidates = Vec::new();
        for remote in configured_remotes {
            if let Some(candidate) = self.remote_head_base_candidate(&remote, &local)? {
                configured_candidates.push(candidate);
            }
        }
        if configured_candidates.len() == 1 {
            return Ok(configured_candidates.remove(0));
        }
        if configured_candidates.len() > 1 {
            let candidates = configured_candidates
                .iter()
                .map(|candidate| {
                    format!(
                        "{}/{}",
                        candidate.remote.as_deref().unwrap_or("?"),
                        candidate.name
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            return Err(anyhow!(
                "ambiguous configured remote HEADs ({candidates}); configure the intended base branch remote explicitly"
            ));
        }
        Ok(local_candidate)
    }

    fn remote_head_base_candidate(
        &self,
        remote: &str,
        local: &[String],
    ) -> Result<Option<BaseBranchCandidate>> {
        if remote == "." || !self.has_remote(remote)? {
            return Ok(None);
        }
        let remote_head_ref = format!("refs/remotes/{remote}/HEAD");
        let output = Command::new("git")
            .current_dir(&self.root)
            .args(["symbolic-ref", "--quiet", "--no-recurse", &remote_head_ref])
            .output()
            .with_context(|| format!("failed to read {remote}/HEAD"))?;

        if output.status.success() {
            let val = String::from_utf8(output.stdout)?.trim().to_string();
            let remote_prefix = format!("refs/remotes/{remote}/");
            if let Some(branch) = val.strip_prefix(&remote_prefix)
                && local.iter().any(|local_branch| local_branch == branch)
            {
                let target = format!("{remote_prefix}{branch}");
                let target_is_symbolic = Command::new("git")
                    .current_dir(&self.root)
                    .args(["symbolic-ref", "--quiet", "--no-recurse", &target])
                    .output()
                    .with_context(|| format!("failed to inspect {remote}/{branch}"))?
                    .status
                    .success();
                if target_is_symbolic || !self.ref_exists(&target)? {
                    return Ok(None);
                }
                return Ok(Some(BaseBranchCandidate {
                    name: branch.to_string(),
                    source: BaseBranchSource::RemoteHead,
                    remote: Some(remote.to_string()),
                }));
            }
        }
        Ok(None)
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
            .arg("--")
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

    pub fn fast_forward_branch(
        &self,
        branch: &str,
        expected_head: &str,
        onto: &str,
    ) -> Result<String> {
        let expected_head = self.resolve_object_id(expected_head)?;
        let onto = self.resolve_object_id(onto)?;
        self.checkout_branch(branch)?;
        if self.head_sha(branch)? != expected_head {
            return Err(anyhow!(
                "base branch '{}' changed after the sync plan was built; rerun sync to review a fresh plan",
                sanitize_terminal_text(branch)
            ));
        }
        self.run(["merge", "--ff-only", "--", &onto])?;
        if self.head_sha(branch)? != onto {
            return Err(anyhow!(
                "base branch '{}' changed while it was being fast-forwarded; rerun sync to review a fresh plan",
                sanitize_terminal_text(branch)
            ));
        }
        Ok(onto)
    }

    pub fn has_remote(&self, name: &str) -> Result<bool> {
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

    pub fn advertised_remote_branch_sha(
        &self,
        remote: &str,
        branch: &str,
    ) -> Result<Option<String>> {
        if !self.has_remote(remote)? {
            return Ok(None);
        }

        let branch_ref = local_branch_ref(branch);
        let output = Command::new("git")
            .current_dir(&self.root)
            .env("GIT_TERMINAL_PROMPT", "0")
            .args([
                "ls-remote",
                "--exit-code",
                "--heads",
                "--refs",
                "--",
                remote,
                &branch_ref,
            ])
            .output()
            .with_context(|| {
                format!(
                    "failed to inspect branch '{}' on remote '{}'",
                    sanitize_terminal_text(branch),
                    sanitize_terminal_text(remote)
                )
            })?;
        if output.status.code() == Some(2) {
            return Ok(None);
        }
        if !output.status.success() {
            return Err(anyhow!(
                "could not inspect branch '{}' on remote '{}'",
                sanitize_terminal_text(branch),
                sanitize_terminal_text(remote)
            ));
        }

        let stdout = String::from_utf8(output.stdout)?;
        let sha = stdout
            .lines()
            .filter_map(|line| line.split_once(char::is_whitespace))
            .find_map(|(sha, reference)| (reference.trim() == branch_ref).then(|| sha.trim()))
            .ok_or_else(|| {
                anyhow!(
                    "remote '{}' returned no object ID for branch '{}'",
                    sanitize_terminal_text(remote),
                    sanitize_terminal_text(branch)
                )
            })?;
        if !matches!(sha.len(), 40 | 64) || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(anyhow!(
                "remote '{}' returned an invalid object ID for branch '{}'",
                sanitize_terminal_text(remote),
                sanitize_terminal_text(branch)
            ));
        }
        Ok(Some(sha.to_ascii_lowercase()))
    }

    pub fn replay_onto(
        &self,
        branch: &str,
        expected_head: &str,
        old_base: &str,
        new_base: &str,
    ) -> Result<String> {
        let expected_head = self.resolve_object_id(expected_head)?;
        let old_base = self.resolve_object_id(old_base)?;
        let new_base = self.resolve_object_id(new_base)?;
        let branch_ref = local_branch_ref(branch);
        let revision_range = format!("{old_base}..{expected_head}");

        self.checkout_branch(branch)?;
        if self.head_sha(branch)? != expected_head {
            return Err(branch_changed_after_sync_plan(branch));
        }
        self.run(["switch", "--detach", "--", &expected_head])?;

        let output = Command::new("git")
            .current_dir(&self.root)
            .args([
                "replay",
                "--onto",
                &new_base,
                "--ref",
                &branch_ref,
                "--ref-action=print",
                "--",
                &revision_range,
            ])
            .output()
            .with_context(|| format!("failed to replay branch '{branch}' from its planned head"))?;
        if !output.status.success() {
            return Err(anyhow!(
                "git replay failed for '{}': {}",
                sanitize_terminal_text(branch),
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let stdout = String::from_utf8(output.stdout)?;
        let mut updates = stdout.lines().filter(|line| !line.trim().is_empty());
        let fields: Vec<&str> = updates
            .next()
            .unwrap_or_default()
            .split_whitespace()
            .collect();
        if fields.len() != 4
            || fields[0] != "update"
            || fields[1] != branch_ref
            || fields[3] != expected_head
            || updates.next().is_some()
            || !valid_object_id(fields[2])
        {
            return Err(anyhow!(
                "git replay returned an unexpected ref update for '{}'",
                sanitize_terminal_text(branch)
            ));
        }

        let replayed_head = fields[2].to_string();
        self.update_local_branch_if_unchanged(branch, &replayed_head, &expected_head)?;
        self.checkout_branch(branch)?;
        Ok(replayed_head)
    }

    pub fn rebase_onto(
        &self,
        branch: &str,
        expected_head: &str,
        old_base: &str,
        new_base: &str,
    ) -> Result<String> {
        let expected_head = self.resolve_object_id(expected_head)?;
        let old_base = self.resolve_object_id(old_base)?;
        let new_base = self.resolve_object_id(new_base)?;
        self.checkout_branch(branch)?;
        if self.head_sha(branch)? != expected_head {
            return Err(branch_changed_after_sync_plan(branch));
        }

        let (temporary_branch, pending_ref) = loop {
            let id = NEXT_RESTACK_ID.fetch_add(1, Ordering::Relaxed);
            let run_id = format!("{}-{id}", std::process::id());
            let candidate = format!("stack/restack/{run_id}/{expected_head}/{branch}");
            let candidate_ref = local_branch_ref(&candidate);
            let pending_ref = restack_pending_ref(&run_id);
            let commands = format!(
                "create {candidate_ref} {expected_head}\ncreate {pending_ref} {expected_head}\n"
            );
            let create = self.run_update_ref_transaction(&commands)?;
            if create.status.success() {
                break (candidate, pending_ref);
            }
            if self.exact_ref_oid(&candidate_ref)?.is_none()
                && self.exact_ref_oid(&pending_ref)?.is_none()
            {
                return Err(anyhow!(
                    "could not create atomic restack recovery state for '{}': {}",
                    sanitize_terminal_text(&candidate),
                    String::from_utf8_lossy(&create.stderr).trim()
                ));
            }
        };

        self.checkout_branch(&temporary_branch)?;
        if let Err(err) = self.run(["rebase", "--onto", &new_base, "--", &old_base]) {
            return Err(anyhow!(
                "restack of '{branch}' stopped on recovery branch '{temporary_branch}': {err}"
            ));
        }
        let rebased_head = self.head_sha(&temporary_branch)?;
        self.run(["switch", "--detach", "--", &rebased_head])?;

        let update_result = self.update_local_branch_and_consume_pending(
            branch,
            &rebased_head,
            &expected_head,
            &pending_ref,
        );
        if update_result.is_err() {
            let temporary_ref = local_branch_ref(&temporary_branch);
            let cleanup = format!(
                "delete {temporary_ref} {rebased_head}\ndelete {pending_ref} {expected_head}\n"
            );
            let _ = self.run_update_ref_transaction(&cleanup);
        } else {
            let _ = self.delete_local_branch_if_unchanged(&temporary_branch, &rebased_head);
        }
        update_result?;
        self.checkout_branch(branch)?;
        Ok(rebased_head)
    }

    pub fn finish_pending_restack(&self) -> Result<Option<String>> {
        let recovery_branch = self.current_branch()?;
        let Some((run_id, expected_head, target_branch)) =
            parse_restack_recovery_branch(&recovery_branch)
        else {
            return Ok(None);
        };
        let pending_ref = restack_pending_ref(run_id);
        if self.exact_ref_oid(&pending_ref)?.as_deref() != Some(expected_head) {
            return Err(anyhow!(
                "refusing to finish untrusted restack recovery branch '{}'",
                sanitize_terminal_text(&recovery_branch)
            ));
        }
        if !self.branch_exists(target_branch)? {
            return Err(anyhow!(
                "cannot finish recovered restack: target branch '{}' no longer exists",
                sanitize_terminal_text(target_branch)
            ));
        }

        let rebased_head = self.head_sha(&recovery_branch)?;
        self.run(["switch", "--detach", "--", &rebased_head])?;
        if let Err(err) = self.update_local_branch_and_consume_pending(
            target_branch,
            &rebased_head,
            expected_head,
            &pending_ref,
        ) {
            let _ = self.checkout_branch(&recovery_branch);
            return Err(err);
        }

        self.checkout_branch(target_branch)?;
        let _ = self.delete_local_branch_if_unchanged(&recovery_branch, &rebased_head);
        Ok(Some(target_branch.to_string()))
    }

    fn update_local_branch_and_consume_pending(
        &self,
        branch: &str,
        new_head: &str,
        expected_head: &str,
        pending_ref: &str,
    ) -> Result<()> {
        let branch_ref = local_branch_ref(branch);
        let commands = format!(
            "update {branch_ref} {new_head} {expected_head}\ndelete {pending_ref} {expected_head}\n"
        );
        let output = self.run_update_ref_transaction(&commands)?;
        if !output.status.success() {
            return Err(branch_changed_after_sync_plan(branch));
        }
        Ok(())
    }

    fn run_update_ref_transaction(&self, commands: &str) -> Result<Output> {
        let mut child = Command::new("git")
            .current_dir(&self.root)
            .args(["update-ref", "--stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to start git update-ref transaction")?;
        child
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow!("git update-ref stdin was unavailable"))?
            .write_all(commands.as_bytes())
            .context("failed to write git update-ref transaction")?;
        child
            .wait_with_output()
            .context("failed to finish git update-ref transaction")
    }

    fn exact_ref_oid(&self, reference: &str) -> Result<Option<String>> {
        let output = Command::new("git")
            .current_dir(&self.root)
            .args([
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                &format!("{reference}^{{commit}}"),
            ])
            .output()
            .with_context(|| format!("failed to inspect exact ref {reference}"))?;
        match output.status.code() {
            Some(0) => Ok(Some(String::from_utf8(output.stdout)?.trim().to_string())),
            Some(1) => Ok(None),
            _ => Err(anyhow!(
                "could not inspect exact ref '{}': {}",
                sanitize_terminal_text(reference),
                String::from_utf8_lossy(&output.stderr).trim()
            )),
        }
    }

    fn update_local_branch_if_unchanged(
        &self,
        branch: &str,
        new_head: &str,
        expected_head: &str,
    ) -> Result<()> {
        let branch_ref = local_branch_ref(branch);
        let output = Command::new("git")
            .current_dir(&self.root)
            .args(["update-ref", &branch_ref, new_head, expected_head])
            .output()
            .with_context(|| format!("failed to conditionally update branch {branch}"))?;
        if !output.status.success() {
            return Err(branch_changed_after_sync_plan(branch));
        }
        Ok(())
    }

    pub fn merge_base(&self, branch: &str, onto: &str) -> Result<String> {
        let branch = self.head_sha(branch)?;
        let onto = self.head_sha(onto)?;
        self.capture(["merge-base", "--", &branch, &onto])
            .map(|s| s.trim().to_string())
    }

    pub fn merge_base_fork_point(&self, parent: &str, child: &str) -> Result<Option<String>> {
        let parent_ref = local_branch_ref(parent);
        let child = self.resolve_object_id(child)?;
        let output = Command::new("git")
            .current_dir(&self.root)
            .args(["merge-base", "--fork-point", &parent_ref, &child])
            .output()
            .with_context(|| format!("failed to find fork point for {parent} -> {child}"))?;
        match output.status.code() {
            Some(0) => {
                let fork_point = String::from_utf8(output.stdout)?.trim().to_string();
                if fork_point.is_empty() {
                    return Err(anyhow!(
                        "git returned an empty fork point for {parent} -> {child}"
                    ));
                }
                Ok(Some(fork_point))
            }
            Some(1) => Ok(None),
            _ => Err(anyhow!(
                "git could not determine a fork point for {parent} -> {child}"
            )),
        }
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

    pub fn is_branch_ancestor(&self, ancestor: &str, branch: &str) -> Result<bool> {
        self.is_ancestor(&self.head_sha(ancestor)?, &self.head_sha(branch)?)
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

    pub fn branch_commit_distance(&self, base: &str, head: &str) -> Result<u32> {
        self.commit_distance(&self.head_sha(base)?, &self.head_sha(head)?)
    }

    fn unambiguous_revision(&self, revision: &str) -> Result<String> {
        if valid_object_id(revision) || revision.starts_with("refs/") {
            Ok(revision.to_string())
        } else if self.branch_exists(revision)? {
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

fn valid_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn branch_changed_after_sync_plan(branch: &str) -> anyhow::Error {
    anyhow!(
        "branch '{}' changed after the sync plan was built; rerun sync to review a fresh plan",
        sanitize_terminal_text(branch)
    )
}

fn parse_restack_recovery_branch(branch: &str) -> Option<(&str, &str, &str)> {
    let tail = branch.strip_prefix("stack/restack/")?;
    let mut parts = tail.splitn(3, '/');
    let run_id = parts.next()?;
    let expected_head = parts.next()?;
    let target_branch = parts.next()?;
    (run_id
        .bytes()
        .all(|byte| byte.is_ascii_digit() || byte == b'-')
        && !run_id.is_empty()
        && valid_object_id(expected_head)
        && !target_branch.is_empty())
    .then_some((run_id, expected_head, target_branch))
}

fn restack_pending_ref(run_id: &str) -> String {
    format!("refs/stack/restacks/{run_id}")
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

    let sanitized = sanitize_terminal_text(raw.trim());
    let mut url = Url::parse(&sanitized).ok()?;
    if !matches!(url.scheme(), "https" | "http") || url.host_str()?.is_empty() {
        return None;
    }
    url.set_username("").ok()?;
    url.set_password(None).ok()?;
    url.set_query(None);
    url.set_fragment(None);

    let path = url.path().trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path).to_string();
    if path.is_empty() || path == "/" {
        return None;
    }
    url.set_path(&path);
    Some(url.to_string())
}

fn sanitize_terminal_text(value: &str) -> String {
    value.chars().filter(|c| !c.is_control()).collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn run_test_git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(root)
            .args(["-c", "commit.gpgSign=false", "-c", "tag.gpgSign=false"])
            .args(args)
            .output()
            .expect("run test git command");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn init_test_repo() -> (tempfile::TempDir, Git) {
        let repo = tempfile::tempdir().expect("test repository");
        run_test_git(repo.path(), &["init", "-b", "main"]);
        let git = Git {
            root: repo.path().to_path_buf(),
        };
        (repo, git)
    }

    fn init_sha256_test_repo() -> (tempfile::TempDir, Git) {
        let repo = tempfile::tempdir().expect("test repository");
        run_test_git(
            repo.path(),
            &["init", "--object-format=sha256", "-b", "main"],
        );
        let git = Git {
            root: repo.path().to_path_buf(),
        };
        (repo, git)
    }

    fn assert_full_oid_wins_over_same_named_branch(repo: &Path, git: &Git) {
        run_test_git(repo, &["config", "user.email", "test@example.com"]);
        run_test_git(repo, &["config", "user.name", "Stack Test"]);
        run_test_git(repo, &["commit", "--allow-empty", "-m", "original"]);
        let original = git.head_sha("main").unwrap();
        run_test_git(repo, &["commit", "--allow-empty", "-m", "new head"]);
        run_test_git(repo, &["branch", &original]);

        assert_eq!(git.resolve_commit(&original).unwrap(), original);
    }

    fn assert_opposite_length_hex_name_resolves_as_branch(repo: &Path, git: &Git, length: usize) {
        run_test_git(repo, &["config", "user.email", "test@example.com"]);
        run_test_git(repo, &["config", "user.name", "Stack Test"]);
        run_test_git(repo, &["commit", "--allow-empty", "-m", "head"]);
        let head = git.head_sha("main").unwrap();
        let branch = "a".repeat(length);
        run_test_git(repo, &["branch", &branch]);

        assert_eq!(git.resolve_commit(&branch).unwrap(), head);
        assert!(git.resolve_object_id(&branch).is_err());
        assert!(!git.object_id_exists(&branch).unwrap());
        assert!(!git.is_first_parent_ancestor(&branch, &head).unwrap());
    }

    #[test]
    fn remote_url_treats_an_option_like_remote_name_as_an_operand() {
        let (repo, git) = init_test_repo();
        run_test_git(
            repo.path(),
            &[
                "config",
                "remote.--all.url",
                "https://github.com/acme/stack-test.git",
            ],
        );
        run_test_git(
            repo.path(),
            &[
                "config",
                "remote.--all.fetch",
                "+refs/heads/*:refs/remotes/--all/*",
            ],
        );

        assert_eq!(
            git.remote_web_url("--all").unwrap().as_deref(),
            Some("https://github.com/acme/stack-test")
        );
    }

    #[test]
    fn fetch_treats_an_option_like_remote_name_as_a_single_operand() {
        let (repo, git) = init_test_repo();
        let remote = tempfile::tempdir().expect("remote repository");
        run_test_git(remote.path(), &["init", "--bare"]);
        run_test_git(
            repo.path(),
            &[
                "config",
                "remote.--all.url",
                remote.path().to_str().expect("remote path"),
            ],
        );
        run_test_git(
            repo.path(),
            &[
                "config",
                "remote.--all.fetch",
                "+refs/heads/*:refs/remotes/--all/*",
            ],
        );
        run_test_git(
            repo.path(),
            &[
                "config",
                "remote.broken.url",
                repo.path()
                    .join("missing.git")
                    .to_str()
                    .expect("missing path"),
            ],
        );
        run_test_git(
            repo.path(),
            &[
                "config",
                "remote.broken.fetch",
                "+refs/heads/*:refs/remotes/broken/*",
            ],
        );

        git.fetch_remote("--all").expect("fetch only --all remote");
    }

    #[test]
    fn sync_remote_selection_preserves_custom_named_forks() {
        let (repo, git) = init_test_repo();
        run_test_git(
            repo.path(),
            &[
                "remote",
                "add",
                "upstream",
                "https://example.invalid/upstream.git",
            ],
        );
        run_test_git(
            repo.path(),
            &[
                "remote",
                "add",
                "company",
                "https://example.invalid/company.git",
            ],
        );

        assert_eq!(git.preferred_sync_remote("origin").unwrap(), "upstream");
        assert_eq!(git.preferred_sync_remote("company").unwrap(), "upstream");
    }

    #[test]
    fn first_parent_history_distinguishes_continuation_from_merged_branch_reuse() {
        let (repo, git) = init_test_repo();
        run_test_git(repo.path(), &["config", "user.email", "test@example.com"]);
        run_test_git(repo.path(), &["config", "user.name", "Stack Test"]);
        run_test_git(repo.path(), &["commit", "--allow-empty", "-m", "base"]);
        run_test_git(repo.path(), &["switch", "-c", "feature"]);
        run_test_git(
            repo.path(),
            &["commit", "--allow-empty", "-m", "feature work"],
        );
        let feature_head = git.head_sha("feature").unwrap();
        run_test_git(repo.path(), &["switch", "main"]);
        run_test_git(repo.path(), &["commit", "--allow-empty", "-m", "main work"]);
        run_test_git(
            repo.path(),
            &["merge", "--no-ff", "feature", "-m", "merge feature"],
        );

        assert!(
            !git.first_parent_commit_oids("main")
                .unwrap()
                .contains(&feature_head)
        );
        assert!(
            git.first_parent_commit_oids("feature")
                .unwrap()
                .contains(&feature_head)
        );
    }

    #[test]
    fn full_sha1_oid_wins_over_a_same_named_branch() {
        let (repo, git) = init_test_repo();
        assert_full_oid_wins_over_same_named_branch(repo.path(), &git);
    }

    #[test]
    fn full_sha256_oid_wins_over_a_same_named_branch() {
        let (repo, git) = init_sha256_test_repo();
        assert_full_oid_wins_over_same_named_branch(repo.path(), &git);
    }

    #[test]
    fn sha1_repo_still_resolves_a_64_hex_branch_name() {
        let (repo, git) = init_test_repo();
        assert_opposite_length_hex_name_resolves_as_branch(repo.path(), &git, 64);
    }

    #[test]
    fn sha256_repo_still_resolves_a_40_hex_branch_name() {
        let (repo, git) = init_sha256_test_repo();
        assert_opposite_length_hex_name_resolves_as_branch(repo.path(), &git, 40);
    }

    #[test]
    fn full_blob_oid_does_not_fall_back_to_a_same_named_branch() {
        let (repo, git) = init_test_repo();
        run_test_git(repo.path(), &["config", "user.email", "test@example.com"]);
        run_test_git(repo.path(), &["config", "user.name", "Stack Test"]);
        run_test_git(repo.path(), &["commit", "--allow-empty", "-m", "head"]);
        std::fs::write(repo.path().join("blob.txt"), "not a commit\n").unwrap();
        let blob_oid = git
            .capture(["hash-object", "-w", "blob.txt"])
            .unwrap()
            .trim()
            .to_string();
        run_test_git(repo.path(), &["branch", &blob_oid]);

        assert!(git.resolve_commit(&blob_oid).is_err());
        assert!(!git.ref_exists(&blob_oid).unwrap());
    }

    #[test]
    fn missing_full_oid_does_not_fall_back_to_a_same_named_branch() {
        let (repo, git) = init_test_repo();
        run_test_git(repo.path(), &["config", "user.email", "test@example.com"]);
        run_test_git(repo.path(), &["config", "user.name", "Stack Test"]);
        run_test_git(repo.path(), &["commit", "--allow-empty", "-m", "head"]);
        let head = git.head_sha("main").unwrap();
        let missing_oid = "f".repeat(40);
        run_test_git(repo.path(), &["branch", &missing_oid]);

        assert!(git.resolve_commit(&missing_oid).is_err());
        assert!(!git.ref_exists(&missing_oid).unwrap());
        assert_eq!(git.head_sha(&missing_oid).unwrap(), head);
    }

    #[test]
    fn merge_base_uses_full_oid_named_branch_tips() {
        let (repo, git) = init_test_repo();
        run_test_git(repo.path(), &["config", "user.email", "test@example.com"]);
        run_test_git(repo.path(), &["config", "user.name", "Stack Test"]);
        run_test_git(repo.path(), &["commit", "--allow-empty", "-m", "original"]);
        let original = git.head_sha("main").unwrap();
        run_test_git(
            repo.path(),
            &["commit", "--allow-empty", "-m", "parent tip"],
        );
        let parent_tip = git.head_sha("main").unwrap();
        run_test_git(repo.path(), &["branch", &original]);
        run_test_git(repo.path(), &["switch", "-c", "child"]);
        run_test_git(repo.path(), &["commit", "--allow-empty", "-m", "child"]);

        assert_eq!(git.merge_base(&original, "child").unwrap(), parent_tip);
    }

    #[test]
    fn explicit_ref_wins_over_a_same_named_local_branch() {
        let (repo, git) = init_test_repo();
        run_test_git(repo.path(), &["config", "user.email", "test@example.com"]);
        run_test_git(repo.path(), &["config", "user.name", "Stack Test"]);
        run_test_git(
            repo.path(),
            &["commit", "--allow-empty", "-m", "remote tip"],
        );
        let remote_tip = git.head_sha("main").unwrap();
        run_test_git(repo.path(), &["commit", "--allow-empty", "-m", "local tip"]);
        run_test_git(
            repo.path(),
            &["update-ref", "refs/remotes/origin/main", &remote_tip],
        );
        run_test_git(repo.path(), &["branch", "refs/remotes/origin/main"]);

        assert_eq!(
            git.resolve_commit("refs/remotes/origin/main").unwrap(),
            remote_tip
        );
    }

    #[test]
    fn stash_restore_uses_the_created_stash_after_an_intervening_stash() {
        let (repo, git) = init_test_repo();
        run_test_git(repo.path(), &["config", "user.email", "test@example.com"]);
        run_test_git(repo.path(), &["config", "user.name", "Stack Test"]);
        std::fs::write(repo.path().join("file.txt"), "initial\n").unwrap();
        run_test_git(repo.path(), &["add", "file.txt"]);
        run_test_git(repo.path(), &["commit", "-m", "initial"]);

        std::fs::write(repo.path().join("file.txt"), "stack changes\n").unwrap();
        let stash = git
            .stash_push("stack-sync-auto-stash")
            .unwrap()
            .expect("stack stash");

        std::fs::write(repo.path().join("file.txt"), "other changes\n").unwrap();
        run_test_git(repo.path(), &["stash", "push", "-m", "intervening stash"]);

        git.stash_restore(&stash).expect("restore stack stash");
        assert_eq!(
            std::fs::read_to_string(repo.path().join("file.txt")).unwrap(),
            "stack changes\n"
        );
        let remaining = git.capture(["stash", "list"]).unwrap();
        assert!(remaining.contains("intervening stash"));
    }

    #[test]
    fn conditional_branch_delete_preserves_an_advanced_ref() {
        let (repo, git) = init_test_repo();
        run_test_git(repo.path(), &["config", "user.email", "test@example.com"]);
        run_test_git(repo.path(), &["config", "user.name", "Stack Test"]);
        std::fs::write(repo.path().join("file.txt"), "initial\n").unwrap();
        run_test_git(repo.path(), &["add", "file.txt"]);
        run_test_git(repo.path(), &["commit", "-m", "initial"]);
        run_test_git(repo.path(), &["branch", "victim"]);
        let expected = git.head_sha("victim").unwrap();

        std::fs::write(repo.path().join("file.txt"), "advanced\n").unwrap();
        run_test_git(repo.path(), &["add", "file.txt"]);
        run_test_git(repo.path(), &["commit", "-m", "advanced"]);
        run_test_git(repo.path(), &["update-ref", "refs/heads/victim", "main"]);
        let advanced = git.head_sha("victim").unwrap();

        let error = git
            .delete_local_branch_if_unchanged("victim", &expected)
            .unwrap_err();
        assert!(error.to_string().contains("changed"));
        assert_eq!(git.head_sha("victim").unwrap(), advanced);
    }

    #[test]
    fn conditional_branch_delete_rejects_a_linked_worktree_checkout() {
        let (repo, git) = init_test_repo();
        run_test_git(repo.path(), &["config", "user.email", "test@example.com"]);
        run_test_git(repo.path(), &["config", "user.name", "Stack Test"]);
        std::fs::write(repo.path().join("file.txt"), "initial\n").unwrap();
        run_test_git(repo.path(), &["add", "file.txt"]);
        run_test_git(repo.path(), &["commit", "-m", "initial"]);
        run_test_git(repo.path(), &["branch", "victim"]);
        let expected = git.head_sha("victim").unwrap();
        let linked_root = tempfile::tempdir().expect("linked worktree root");
        let linked = linked_root.path().join("linked");
        run_test_git(
            repo.path(),
            &[
                "worktree",
                "add",
                linked.to_str().expect("linked path"),
                "victim",
            ],
        );

        let error = git
            .delete_local_branch_if_unchanged("victim", &expected)
            .unwrap_err();
        assert!(error.to_string().contains("checked out"));
        assert_eq!(git.head_sha("victim").unwrap(), expected);
    }

    #[test]
    fn conditional_branch_delete_removes_branch_configuration() {
        let (repo, git) = init_test_repo();
        run_test_git(repo.path(), &["config", "user.email", "test@example.com"]);
        run_test_git(repo.path(), &["config", "user.name", "Stack Test"]);
        std::fs::write(repo.path().join("file.txt"), "initial\n").unwrap();
        run_test_git(repo.path(), &["add", "file.txt"]);
        run_test_git(repo.path(), &["commit", "-m", "initial"]);
        run_test_git(repo.path(), &["branch", "victim"]);
        run_test_git(repo.path(), &["config", "branch.victim.remote", "origin"]);
        run_test_git(
            repo.path(),
            &["config", "branch.victim.merge", "refs/heads/victim"],
        );
        let expected = git.head_sha("victim").unwrap();

        git.delete_local_branch_if_unchanged("victim", &expected)
            .unwrap();

        assert!(!git.branch_exists("victim").unwrap());
        let config = Command::new("git")
            .current_dir(repo.path())
            .args(["config", "--get", "branch.victim.remote"])
            .output()
            .unwrap();
        assert!(!config.status.success());
    }

    #[test]
    fn conditional_branch_delete_preserves_prefix_namesake_configuration() {
        let (repo, git) = init_test_repo();
        run_test_git(repo.path(), &["config", "user.email", "test@example.com"]);
        run_test_git(repo.path(), &["config", "user.name", "Stack Test"]);
        std::fs::write(repo.path().join("file.txt"), "initial\n").unwrap();
        run_test_git(repo.path(), &["add", "file.txt"]);
        run_test_git(repo.path(), &["commit", "-m", "initial"]);
        run_test_git(repo.path(), &["branch", "foo"]);
        run_test_git(repo.path(), &["branch", "foo.bar"]);
        run_test_git(
            repo.path(),
            &["config", "branch.foo.bar.remote", "namesake"],
        );
        let expected = git.head_sha("foo").unwrap();

        git.delete_local_branch_if_unchanged("foo", &expected)
            .unwrap();

        let config = git
            .capture(["config", "--get", "branch.foo.bar.remote"])
            .unwrap();
        assert_eq!(config.trim(), "namesake");
    }

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

    #[test]
    fn parse_remote_to_web_url_redacts_https_credentials() {
        let parsed = parse_remote_to_web_url(
            "https://build-user:ghp_super-secret%40value@github.com/acme/repo.git",
        )
        .expect("url should parse");
        assert_eq!(parsed, "https://github.com/acme/repo");
    }

    #[test]
    fn parse_remote_to_web_url_drops_query_and_fragment_credentials() {
        let parsed = parse_remote_to_web_url(
            "https://github.com:8443/acme/repo.git?access_token=super-secret#fragment",
        )
        .expect("url should parse");
        assert_eq!(parsed, "https://github.com:8443/acme/repo");
    }
}
