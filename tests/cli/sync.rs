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
