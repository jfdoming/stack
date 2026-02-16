use std::fs;
use std::path::Path;
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
    cmd
}

#[test]
fn create_command_creates_branch_and_persists_parent_link() {
    let repo = init_repo();

    stack_cmd(repo.path())
        .args([
            "create",
            "--parent",
            "main",
            "--name",
            "feat/one",
            "--porcelain",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"created\": \"feat/one\""))
        .stdout(predicate::str::contains(
            "\"create_url\": \"https://github.com/acme/stack-test/compare/main...feat/one?expand=1\"",
        ));

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(db_path).expect("open db");

    let child_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM branches WHERE name = 'feat/one'",
            [],
            |row| row.get(0),
        )
        .expect("child count");
    assert_eq!(child_count, 1);

    let parent_name: String = conn
        .query_row(
            "SELECT p.name
             FROM branches c
             JOIN branches p ON p.id = c.parent_branch_id
             WHERE c.name = 'feat/one'",
            [],
            |row| row.get(0),
        )
        .expect("parent name");
    assert_eq!(parent_name, "main");
}

#[test]
fn stack_without_args_prints_plain_tree_in_non_tty() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/tree"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("feat/tree"));
}

#[test]
fn sync_dry_run_porcelain_reports_restack_operation() {
    let repo = init_repo();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/parent"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/parent", "--name", "feat/child"])
        .assert()
        .success();

    let old_parent_sha = {
        let output = Command::new("git")
            .current_dir(repo.path())
            .args(["rev-parse", "feat/parent"])
            .output()
            .expect("rev-parse parent");
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .expect("utf8")
            .trim()
            .to_string()
    };

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(&db_path).expect("open db");
    conn.execute(
        "UPDATE branches SET last_synced_head_sha = ?1 WHERE name = 'feat/parent'",
        [old_parent_sha],
    )
    .expect("seed last synced sha");

    run_git(repo.path(), &["checkout", "feat/parent"]);
    fs::write(repo.path().join("parent.txt"), "parent update\n").expect("write parent change");
    run_git(repo.path(), &["add", "parent.txt"]);
    run_git(repo.path(), &["commit", "-m", "parent update"]);
    run_git(repo.path(), &["checkout", "main"]);

    let output = stack_cmd(repo.path())
        .args(["sync", "--dry-run", "--porcelain"])
        .output()
        .expect("run stack sync");
    assert!(
        output.status.success(),
        "sync failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let ops = json["operations"].as_array().expect("operations array");
    let found = ops.iter().any(|op| {
        op["kind"] == "restack" && op["branch"] == "feat/child" && op["onto"] == "feat/parent"
    });
    assert!(found, "expected restack op for feat/child onto feat/parent");
}

#[test]
fn pr_dry_run_uses_parent_branch_as_base() {
    let repo = init_repo();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/parent"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/parent", "--name", "feat/child"])
        .assert()
        .success();

    run_git(repo.path(), &["checkout", "feat/child"]);

    let output = stack_cmd(repo.path())
        .args(["pr", "--dry-run", "--porcelain"])
        .output()
        .expect("run stack pr --dry-run");
    assert!(output.status.success());

    let json: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(json["head"], "feat/child");
    assert_eq!(json["base"], "feat/parent");
}

#[test]
fn pr_dry_run_fails_when_current_branch_is_not_tracked() {
    let repo = init_repo();
    run_git(repo.path(), &["checkout", "-b", "feat/untracked"]);

    stack_cmd(repo.path())
        .args(["pr", "--dry-run"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("is not tracked"));
}

#[test]
fn pr_dry_run_fails_when_current_branch_has_no_tracked_parent() {
    let repo = init_repo();
    run_git(repo.path(), &["checkout", "-b", "feat/orphan"]);
    stack_cmd(repo.path()).assert().success();

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(db_path).expect("open db");
    conn.execute(
        "INSERT INTO branches(name, parent_branch_id) VALUES ('feat/orphan', NULL)",
        [],
    )
    .expect("insert orphan tracked branch");

    stack_cmd(repo.path())
        .args(["pr", "--dry-run"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("has no tracked parent"));
}

#[cfg(unix)]
#[test]
fn pr_does_not_create_when_existing_pr_is_found() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/existing"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "feat/existing"]);

    let fake_bin = repo.path().join("fake-bin");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        "#!/usr/bin/env bash\nif [[ \"$*\" == *\"pr list\"* ]] && [[ \"$*\" == *\"--head feat/existing\"* ]]; then\n  echo '[{\"number\": 77, \"state\": \"OPEN\", \"baseRefName\": \"main\", \"mergeCommit\": null}]'\n  exit 0\nfi\nif [[ \"$*\" == *\"pr create\"* ]]; then\n  echo 'create should not be called' >&2\n  exit 1\nfi\necho '[]'\n",
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");

    let current_path = env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", fake_bin.display(), current_path);

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["pr"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "PR already exists for 'feat/existing': #77",
        ));
}

#[cfg(unix)]
#[test]
fn pr_porcelain_reports_existing_pr_without_create() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/existing-json"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "feat/existing-json"]);

    let fake_bin = repo.path().join("fake-bin");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        "#!/usr/bin/env bash\nif [[ \"$*\" == *\"pr list\"* ]] && [[ \"$*\" == *\"--head feat/existing-json\"* ]]; then\n  echo '[{\"number\": 88, \"state\": \"OPEN\", \"baseRefName\": \"main\", \"mergeCommit\": null}]'\n  exit 0\nfi\nif [[ \"$*\" == *\"pr create\"* ]]; then\n  echo 'create should not be called' >&2\n  exit 1\nfi\necho '[]'\n",
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");

    let current_path = env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", fake_bin.display(), current_path);

    let output = stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["pr", "--porcelain"])
        .output()
        .expect("run stack pr porcelain");
    assert!(output.status.success());

    let json: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(json["existing_pr_number"], 88);
    assert_eq!(json["will_create"], false);
}

#[cfg(unix)]
#[test]
fn pr_yes_allows_non_interactive_creation() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/pr-create"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "feat/pr-create"]);

    let fake_bin = repo.path().join("fake-bin");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        "#!/usr/bin/env bash\nif [[ \"$*\" == *\"pr list\"* ]] && [[ \"$*\" == *\"--head feat/pr-create\"* ]]; then\n  echo '[]'\n  exit 0\nfi\nif [[ \"$*\" == *\"pr create\"* ]]; then\n  echo 'https://github.com/acme/stack-test/pull/99'\n  exit 0\nfi\necho '[]'\n",
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");

    let current_path = env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", fake_bin.display(), current_path);

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["--yes", "pr"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "created PR: https://github.com/acme/stack-test/pull/99",
        ));
}

#[cfg(unix)]
#[test]
fn pr_create_fails_when_gh_returns_non_zero() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/pr-fail"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "feat/pr-fail"]);

    let fake_bin = repo.path().join("fake-bin");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        "#!/usr/bin/env bash\necho 'auth failed' >&2\nexit 1\n",
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");

    let current_path = env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", fake_bin.display(), current_path);

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["--yes", "pr"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("gh command failed"));
}

#[test]
fn pr_requires_yes_in_non_interactive_mode_before_create() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/pr-confirm"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "feat/pr-confirm"]);

    stack_cmd(repo.path())
        .args(["pr"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "confirmation required in non-interactive mode",
        ));
}

#[cfg(unix)]
#[test]
fn track_infer_uses_fork_qualified_head_for_pr_detection() {
    let repo = init_repo();
    run_git(repo.path(), &["checkout", "-b", "feat/fork-pr"]);
    run_git(repo.path(), &["checkout", "main"]);

    run_git(
        repo.path(),
        &[
            "remote",
            "set-url",
            "origin",
            "git@github.com:alice/stack-test.git",
        ],
    );
    run_git(
        repo.path(),
        &["config", "branch.feat/fork-pr.remote", "origin"],
    );

    let fake_bin = repo.path().join("fake-bin");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        "#!/usr/bin/env bash\nif [[ \"$*\" == *\"--head feat/fork-pr\"* ]]; then\n  echo '[]'\n  exit 0\nfi\nif [[ \"$*\" == *\"--head alice:feat/fork-pr\"* ]]; then\n  echo '[{\"number\": 42, \"state\": \"OPEN\", \"baseRefName\": \"main\", \"mergeCommit\": null}]'\n  exit 0\nfi\necho '[]'\n",
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");

    let current_path = env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", fake_bin.display(), current_path);

    let output = stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["track", "feat/fork-pr", "--dry-run", "--porcelain"])
        .output()
        .expect("run stack track infer dry-run");
    assert!(output.status.success());

    let json: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(json["changes"][0]["branch"], "feat/fork-pr");
    assert_eq!(json["changes"][0]["new_parent"], "main");
    assert_eq!(json["changes"][0]["source"], "pr_base");
}

#[test]
fn stack_default_output_includes_pr_hyperlink_when_cached_pr_exists() {
    let repo = init_repo();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/link"])
        .assert()
        .success();

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(&db_path).expect("open db");
    conn.execute(
        "UPDATE branches SET cached_pr_number = 123, cached_pr_state = 'open' WHERE name = 'feat/link'",
        [],
    )
    .expect("seed pr number");

    stack_cmd(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "https://github.com/acme/stack-test/pull/123",
        ));
}

#[test]
fn sync_succeeds_without_origin_remote() {
    let repo = init_repo_without_origin();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/local"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .args(["sync", "--yes"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "no 'origin' remote configured; skipping fetch",
        ));
}

#[test]
fn stack_default_output_includes_pr_creation_link_when_pr_missing() {
    let repo = init_repo();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/new-pr"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "https://github.com/acme/stack-test/compare/main...feat/new-pr?expand=1",
        ));
}

#[test]
fn completions_command_generates_script() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["completions", "zsh"])
        .assert()
        .success()
        .stdout(predicate::str::contains("_stack"));
}

#[test]
fn completions_without_shell_in_non_interactive_mode_requires_argument() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["completions"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "shell required in non-interactive mode",
        ));
}

#[test]
fn sync_plan_fetch_uses_base_branch_remote() {
    let repo = init_repo_with_named_remote("upstream");

    let output = stack_cmd(repo.path())
        .args(["sync", "--dry-run", "--porcelain"])
        .output()
        .expect("run stack sync");
    assert!(output.status.success());

    let json: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let ops = json["operations"].as_array().expect("operations array");
    let fetch = ops.first().expect("has first op");
    assert_eq!(fetch["kind"], "fetch");
    assert_eq!(fetch["branch"], "upstream");
}

#[test]
fn delete_command_splices_children_and_deletes_local_branch() {
    let repo = init_repo();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/a"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/a", "--name", "feat/b"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/b", "--name", "feat/c"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .args(["--yes", "delete", "feat/b"])
        .assert()
        .success();

    let branch_list = Command::new("git")
        .current_dir(repo.path())
        .args(["branch", "--list", "feat/b"])
        .output()
        .expect("git branch list");
    assert!(branch_list.status.success());
    let listed = String::from_utf8(branch_list.stdout).expect("utf8");
    assert!(listed.trim().is_empty(), "feat/b should be deleted");

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(&db_path).expect("open db");
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM branches WHERE name = 'feat/b'",
            [],
            |row| row.get(0),
        )
        .expect("query count");
    assert_eq!(count, 0);

    let parent_name: String = conn
        .query_row(
            "SELECT p.name
             FROM branches c
             JOIN branches p ON p.id = c.parent_branch_id
             WHERE c.name = 'feat/c'",
            [],
            |row| row.get(0),
        )
        .expect("query feat/c parent");
    assert_eq!(parent_name, "feat/a");
}

#[test]
fn untrack_command_splices_children_and_removes_branch_record() {
    let repo = init_repo();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/a"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/a", "--name", "feat/b"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/b", "--name", "feat/c"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .args(["untrack", "feat/b", "--porcelain"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"action\": \"untrack\""));

    let output = stack_cmd(repo.path())
        .args(["--porcelain"])
        .output()
        .expect("run stack --porcelain");
    assert!(output.status.success());
    let branches: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let rows = branches.as_array().expect("branch array");

    assert!(
        !rows.iter().any(|row| row["name"] == "feat/b"),
        "feat/b should be untracked"
    );

    let feat_c = rows
        .iter()
        .find(|row| row["name"] == "feat/c")
        .expect("feat/c entry");
    assert_eq!(feat_c["parent"], "feat/a");
}

#[test]
fn untrack_without_branch_in_non_interactive_mode_assumes_only_viable_branch() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/a"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .args(["--yes", "untrack"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "assuming target branch 'feat/a' (only viable branch)",
        ));
}

#[test]
fn untrack_without_branch_in_non_interactive_mode_requires_argument_when_multiple_tracked() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/a"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/b"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .args(["untrack"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "branch required in non-interactive mode",
        ));
}

#[test]
fn unlink_subcommand_is_removed() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["unlink", "feat/a"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unrecognized subcommand 'unlink'"));
}

#[test]
fn create_without_parent_in_non_interactive_mode_assumes_only_viable_branch() {
    let repo = init_repo();

    stack_cmd(repo.path())
        .args(["create", "--name", "feat/a"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "assuming parent branch 'main' (only viable branch)",
        ));
}

#[test]
fn delete_without_branch_in_non_interactive_mode_assumes_only_viable_branch() {
    let repo = init_repo();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/a"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .args(["delete", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "assuming target branch 'feat/a' (only viable branch)",
        ));
}

#[test]
fn delete_without_branch_in_non_interactive_mode_requires_argument_when_multiple_tracked() {
    let repo = init_repo();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/a"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/b"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .args(["delete", "--dry-run"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "branch required in non-interactive mode",
        ));
}

#[test]
fn track_command_sets_parent_for_existing_branch() {
    let repo = init_repo();
    run_git(repo.path(), &["checkout", "-b", "feat/a"]);
    run_git(repo.path(), &["checkout", "main"]);

    let output = stack_cmd(repo.path())
        .args(["track", "feat/a", "--parent", "main", "--porcelain"])
        .output()
        .expect("run stack track");
    assert!(output.status.success());

    let json: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(json["mode"], "single");
    assert_eq!(json["applied"], true);
    assert_eq!(json["changes"][0]["branch"], "feat/a");
    assert_eq!(json["changes"][0]["new_parent"], "main");
    assert_eq!(json["changes"][0]["source"], "explicit");
}

#[test]
fn track_without_branch_in_non_interactive_mode_assumes_only_viable_branch() {
    let repo = init_repo();
    run_git(repo.path(), &["checkout", "-b", "feat/a"]);
    run_git(repo.path(), &["checkout", "main"]);

    stack_cmd(repo.path())
        .args(["track", "--parent", "main", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "assuming target branch 'feat/a' (only viable branch)",
        ));
}

#[test]
fn track_without_branch_in_non_interactive_mode_requires_yes_before_actioning_assumed_target() {
    let repo = init_repo();
    run_git(repo.path(), &["checkout", "-b", "feat/a"]);
    run_git(repo.path(), &["checkout", "main"]);

    stack_cmd(repo.path())
        .args(["track", "--parent", "main"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "target branch was auto-selected as 'feat/a'",
        ));

    stack_cmd(repo.path())
        .args(["--yes", "track", "--parent", "main"])
        .assert()
        .success();
}

#[test]
fn untrack_without_branch_in_non_interactive_mode_requires_yes_before_actioning_assumed_target() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/a"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .args(["untrack"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "target branch was auto-selected as 'feat/a'",
        ));
}

#[test]
fn track_without_branch_in_non_interactive_mode_requires_argument_when_multiple_branches() {
    let repo = init_repo();
    run_git(repo.path(), &["checkout", "-b", "feat/a"]);
    run_git(repo.path(), &["checkout", "main"]);
    run_git(repo.path(), &["checkout", "-b", "feat/b"]);
    run_git(repo.path(), &["checkout", "main"]);

    stack_cmd(repo.path())
        .args(["track", "--parent", "main", "--dry-run"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "branch required in non-interactive mode",
        ));
}

#[test]
fn track_without_parent_in_non_interactive_mode_infers_parent_by_default() {
    let repo = init_repo();
    run_git(repo.path(), &["checkout", "-b", "feat/a"]);
    run_git(repo.path(), &["checkout", "main"]);

    stack_cmd(repo.path())
        .args(["track", "feat/a", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "would track 'feat/a' under 'main'",
        ));
}

#[test]
fn track_without_parent_in_non_interactive_mode_uses_inference_when_possible() {
    let repo = init_repo();
    run_git(repo.path(), &["checkout", "-b", "feat/a"]);
    fs::write(repo.path().join("a.txt"), "a\n").expect("write a");
    run_git(repo.path(), &["add", "a.txt"]);
    run_git(repo.path(), &["commit", "-m", "a"]);
    run_git(repo.path(), &["checkout", "-b", "feat/b"]);
    run_git(repo.path(), &["checkout", "main"]);

    stack_cmd(repo.path())
        .args(["track", "feat/b", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "would track 'feat/b' under 'feat/a'",
        ));
}

#[test]
fn track_infer_dry_run_reports_inferred_parent() {
    let repo = init_repo();
    run_git(repo.path(), &["checkout", "-b", "feat/a"]);
    fs::write(repo.path().join("a.txt"), "a\n").expect("write a");
    run_git(repo.path(), &["add", "a.txt"]);
    run_git(repo.path(), &["commit", "-m", "a"]);
    run_git(repo.path(), &["checkout", "-b", "feat/b"]);
    fs::write(repo.path().join("b.txt"), "b\n").expect("write b");
    run_git(repo.path(), &["add", "b.txt"]);
    run_git(repo.path(), &["commit", "-m", "b"]);
    run_git(repo.path(), &["checkout", "main"]);

    let output = stack_cmd(repo.path())
        .args(["track", "feat/b", "--infer", "--dry-run", "--porcelain"])
        .output()
        .expect("run stack track infer dry-run");
    assert!(output.status.success());
    let json: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(json["applied"], false);
    assert_eq!(json["changes"][0]["branch"], "feat/b");
    assert_eq!(json["changes"][0]["new_parent"], "feat/a");
}

#[test]
fn track_conflict_in_non_interactive_mode_requires_force() {
    let repo = init_repo();
    run_git(repo.path(), &["checkout", "-b", "feat/a"]);
    run_git(repo.path(), &["checkout", "main"]);
    run_git(repo.path(), &["checkout", "-b", "feat/c"]);
    run_git(repo.path(), &["checkout", "main"]);

    stack_cmd(repo.path())
        .args(["track", "feat/c", "--parent", "feat/a"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .args(["track", "feat/c", "--parent", "main"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("use --force"));

    stack_cmd(repo.path())
        .args(["track", "feat/c", "--parent", "main", "--force"])
        .assert()
        .success();
}

#[test]
fn track_all_non_interactive_errors_when_unresolved() {
    let repo = init_repo();
    run_git(repo.path(), &["checkout", "-b", "feat/a"]);
    run_git(repo.path(), &["checkout", "main"]);
    run_git(repo.path(), &["checkout", "-b", "feat/b"]);
    run_git(repo.path(), &["checkout", "main"]);

    stack_cmd(repo.path())
        .args(["track", "--all"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "some branches could not be resolved",
        ));
}
