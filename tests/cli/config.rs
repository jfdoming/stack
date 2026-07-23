#[test]
fn config_push_target_persists_and_reports_repository_policy() {
    let repo = init_repo();

    stack_cmd(repo.path())
        .args(["config", "push-target", "upstream"])
        .assert()
        .success()
        .stdout(predicate::str::contains("push target: upstream"));

    let output = stack_cmd(repo.path())
        .args(["--porcelain", "config", "push-target"])
        .output()
        .expect("read push target config");
    assert!(output.status.success());
    let json: Value = serde_json::from_slice(&output.stdout).expect("valid config json");
    assert_eq!(json["push_target"], "upstream");

    let conn = Connection::open(repo.path().join(".git/stack.db")).expect("open stack db");
    let stored: Option<String> = conn
        .query_row("SELECT push_target FROM repo_meta WHERE id = 1", [], |row| {
            row.get(0)
        })
        .expect("read stored push target");
    assert_eq!(stored.as_deref(), Some("upstream"));
}

#[test]
fn config_push_target_auto_clears_cached_detection() {
    let repo = init_repo();
    stack_cmd(repo.path()).assert().success();
    let conn = Connection::open(repo.path().join(".git/stack.db")).expect("open stack db");
    conn.execute(
        "UPDATE repo_meta
         SET push_target = 'fork', canonical_repo = 'acme/stack-test',
             fork_repo = 'alice/stack-test', push_permission = 'WRITE',
             permission_checked_at = 123
         WHERE id = 1",
        [],
    )
    .expect("seed placement cache");
    drop(conn);

    stack_cmd(repo.path())
        .args(["config", "push-target", "auto"])
        .assert()
        .success();

    let conn = Connection::open(repo.path().join(".git/stack.db")).expect("open stack db");
    let row: (String, Option<String>, Option<i64>) = conn
        .query_row(
            "SELECT push_target, push_permission, permission_checked_at FROM repo_meta WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("read cleared cache");
    assert_eq!(row, ("auto".to_string(), None, None));
}

#[test]
fn config_status_distinguishes_fetch_and_push_repository_urls() {
    let repo = init_repo();
    run_git(
        repo.path(),
        &["remote", "set-url", "origin", "git@github.com:acme/stack-test.git"],
    );
    run_git(
        repo.path(),
        &[
            "remote",
            "set-url",
            "--push",
            "origin",
            "git@github.com:alice/stack-test.git",
        ],
    );

    let output = stack_cmd(repo.path())
        .args(["--porcelain", "config", "push-target"])
        .output()
        .expect("read split-url config");
    assert!(output.status.success());
    let json: Value = serde_json::from_slice(&output.stdout).expect("valid config json");
    assert_eq!(json["canonical_repository"], "acme/stack-test");
    assert_eq!(json["fork_repository"], "alice/stack-test");
    assert_eq!(json["fork_remote"], "origin");
    assert!(json["canonical_remote"].is_null());
}

#[test]
fn config_status_reports_resolved_target_and_cache_state() {
    let repo = init_repo();
    stack_cmd(repo.path()).assert().success();
    let conn = Connection::open(repo.path().join(".git/stack.db")).expect("open stack db");
    conn.execute(
        "UPDATE repo_meta
         SET push_target = 'auto', canonical_repo = 'acme/stack-test',
             push_permission = 'WRITE', permission_checked_at = strftime('%s', 'now')
         WHERE id = 1",
        [],
    )
    .expect("seed fresh placement cache");
    drop(conn);

    stack_cmd(repo.path())
        .args(["config", "push-target"])
        .assert()
        .success()
        .stdout(predicate::str::contains("resolved target: upstream"))
        .stdout(predicate::str::contains("cached detection: fresh"));

    let output = stack_cmd(repo.path())
        .args(["--porcelain", "config", "push-target"])
        .output()
        .expect("read cache status");
    assert!(output.status.success());
    let json: Value = serde_json::from_slice(&output.stdout).expect("valid config json");
    assert_eq!(json["cache_state"], "fresh");
    assert!(json["cache_age_seconds"].as_i64().is_some_and(|age| age >= 0));
}
