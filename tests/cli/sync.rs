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
fn sync_restores_branch_checked_out_before_run() {
    let repo = init_repo_without_origin();

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

    stack_cmd(repo.path())
        .args(["sync", "--yes"])
        .assert()
        .success();

    let branch_output = Command::new("git")
        .current_dir(repo.path())
        .args(["branch", "--show-current"])
        .output()
        .expect("read current branch");
    assert!(branch_output.status.success());
    assert_eq!(
        String::from_utf8(branch_output.stdout)
            .expect("utf8")
            .trim(),
        "main"
    );
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

#[cfg(unix)]
#[test]
fn sync_updates_existing_pr_body_with_managed_section() {
    let repo = init_repo_without_origin();
    run_git(
        repo.path(),
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:acme/stack-test.git",
        ],
    );
    run_git(repo.path(), &["config", "branch.main.remote", "no-fetch"]);

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/parent"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/parent", "--name", "feat/child"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "main"]);

    let fake_bin = repo.path().join("fake-bin");
    let gh_log = repo.path().join("gh.log");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        format!(
            "#!/usr/bin/env bash\necho \"$@\" >> '{}'\nif [[ \"$1\" == \"pr\" && \"$2\" == \"list\" ]]; then\n  if [[ \"$*\" == *\"headRefName\"* ]]; then\n    echo '[{{\"number\":42,\"state\":\"OPEN\",\"baseRefName\":\"feat/parent\",\"headRefName\":\"feat/child\",\"mergeCommit\":null,\"body\":\"Existing reviewer notes\"}}]'\n    exit 0\n  fi\n  for ((i=1; i<=$#; i++)); do\n    if [[ \"${{!i}}\" == \"--head\" ]]; then\n      next=$((i+1))\n      head=\"${{!next}}\"\n      break\n    fi\n  done\n  if [[ \"$head\" == \"feat/child\" ]]; then\n    echo '[{{\"number\":42,\"state\":\"OPEN\",\"baseRefName\":\"feat/parent\",\"headRefName\":\"feat/child\",\"mergeCommit\":null,\"body\":\"Existing reviewer notes\"}}]'\n  else\n    echo '[]'\n  fi\n  exit 0\nfi\nif [[ \"$1\" == \"pr\" && \"$2\" == \"edit\" ]]; then\n  exit 0\nfi\necho '[]'\n",
            gh_log.display()
        ),
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");

    let current_path = env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", fake_bin.display(), current_path);

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["sync", "--yes"])
        .assert()
        .success();

    let gh_calls = fs::read_to_string(&gh_log).expect("read gh log");
    assert!(
        gh_calls.contains("pr edit 42 --body"),
        "expected pr edit call for managed body refresh, got: {gh_calls}"
    );
    assert!(
        gh_calls.contains("pr list --state all --limit 200"),
        "expected batched pr list metadata request, got: {gh_calls}"
    );
    assert!(
        gh_calls.contains("stack:managed:start"),
        "expected managed section start marker in edited body, got: {gh_calls}"
    );
    assert!(
        gh_calls.contains("feat/parent"),
        "expected parent reference in edited body, got: {gh_calls}"
    );
}
