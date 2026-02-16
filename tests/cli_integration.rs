use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(unix)]
use std::{env, os::unix::fs::PermissionsExt};

use assert_cmd::prelude::*;
use predicates::prelude::*;
use rusqlite::Connection;
use serde_json::Value;
use tempfile::TempDir;

fn init_repo() -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    run_git(dir.path(), &["init", "-b", "main"]);
    run_git(dir.path(), &["config", "user.email", "test@example.com"]);
    run_git(dir.path(), &["config", "user.name", "Stack Test"]);
    run_git(dir.path(), &["config", "commit.gpgsign", "false"]);

    fs::write(dir.path().join("README.md"), "init\n").expect("write readme");
    run_git(dir.path(), &["add", "README.md"]);
    run_git(dir.path(), &["commit", "-m", "initial"]);
    run_git(
        dir.path(),
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:acme/stack-test.git",
        ],
    );

    dir
}

fn init_repo_without_origin() -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    run_git(dir.path(), &["init", "-b", "main"]);
    run_git(dir.path(), &["config", "user.email", "test@example.com"]);
    run_git(dir.path(), &["config", "user.name", "Stack Test"]);
    run_git(dir.path(), &["config", "commit.gpgsign", "false"]);

    fs::write(dir.path().join("README.md"), "init\n").expect("write readme");
    run_git(dir.path(), &["add", "README.md"]);
    run_git(dir.path(), &["commit", "-m", "initial"]);
    dir
}

fn init_repo_with_named_remote(remote: &str) -> TempDir {
    let dir = init_repo_without_origin();
    run_git(
        dir.path(),
        &[
            "remote",
            "add",
            remote,
            "git@github.com:acme/stack-test.git",
        ],
    );
    run_git(dir.path(), &["config", "branch.main.remote", remote]);
    run_git(
        dir.path(),
        &["config", "branch.main.merge", "refs/heads/main"],
    );
    dir
}

fn configure_local_push_url(repo: &Path) -> PathBuf {
    let bare = repo.join("origin-push.git");
    run_git(repo, &["init", "--bare", bare.to_str().expect("bare path")]);
    run_git(
        repo,
        &[
            "remote",
            "set-url",
            "--push",
            "origin",
            bare.to_str().expect("bare path"),
        ],
    );
    run_git(repo, &["push", "--set-upstream", "origin", "main"]);
    bare
}

#[cfg(unix)]
fn install_fake_browser_openers(fake_bin: &Path, log_path: &Path) {
    let script = format!(
        "#!/usr/bin/env bash\necho \"$@\" >> '{}'\nexit 0\n",
        log_path.display()
    );
    for bin in ["xdg-open", "open"] {
        let path = fake_bin.join(bin);
        fs::write(&path, &script).expect("write fake browser opener");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .expect("chmod fake browser opener");
    }
}

fn run_git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn stack_cmd(repo: &Path) -> Command {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("stack"));
    cmd.current_dir(repo);
    cmd.env("NO_COLOR", "1");
    cmd.env("CLICOLOR", "0");
    cmd.env("STACK_MOCK_BROWSER_OPEN", "1");
    cmd
}

include!("cli/create.rs");
include!("cli/stack.rs");
include!("cli/sync.rs");
include!("cli/pr.rs");
include!("cli/completions.rs");
include!("cli/delete.rs");
include!("cli/doctor.rs");
include!("cli/untrack.rs");
include!("cli/track.rs");
