use std::fs;
use std::path::Path;
use std::process::Command;

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
