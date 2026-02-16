#[test]
fn doctor_fix_clears_parent_link_on_base_branch() {
    let repo = init_repo();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/a"])
        .assert()
        .success();

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(&db_path).expect("open db");
    conn.execute(
        "UPDATE branches
         SET parent_branch_id = (SELECT id FROM branches WHERE name = 'feat/a')
         WHERE name = 'main'",
        [],
    )
    .expect("seed invalid base parent link");

    stack_cmd(repo.path())
        .args(["doctor", "--porcelain"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"code\": \"base_has_parent\""));

    stack_cmd(repo.path())
        .args(["doctor", "--fix"])
        .assert()
        .success();

    let parent_id: Option<i64> = conn
        .query_row(
            "SELECT parent_branch_id FROM branches WHERE name = 'main'",
            [],
            |row| row.get(0),
        )
        .expect("query main parent id");
    assert!(parent_id.is_none(), "expected base branch parent to be cleared");
}

#[test]
fn doctor_fix_clears_incomplete_pr_cache_fields() {
    let repo = init_repo();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/a"])
        .assert()
        .success();

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(&db_path).expect("open db");
    conn.execute(
        "UPDATE branches
         SET cached_pr_number = 99, cached_pr_state = NULL
         WHERE name = 'feat/a'",
        [],
    )
    .expect("seed incomplete pr cache");

    stack_cmd(repo.path())
        .args(["doctor", "--porcelain"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"code\": \"incomplete_pr_cache\""));

    stack_cmd(repo.path())
        .args(["doctor", "--fix"])
        .assert()
        .success();

    let row: (Option<i64>, Option<String>) = conn
        .query_row(
            "SELECT cached_pr_number, cached_pr_state FROM branches WHERE name = 'feat/a'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("query pr cache");
    assert_eq!(row.0, None);
    assert_eq!(row.1, None);
}

#[test]
fn doctor_fix_breaks_parent_cycles() {
    let repo = init_repo();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/a"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/a", "--name", "feat/b"])
        .assert()
        .success();

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(&db_path).expect("open db");
    conn.execute(
        "UPDATE branches
         SET parent_branch_id = (SELECT id FROM branches WHERE name = 'feat/b')
         WHERE name = 'feat/a'",
        [],
    )
    .expect("seed cycle a -> b");
    conn.execute(
        "UPDATE branches
         SET parent_branch_id = (SELECT id FROM branches WHERE name = 'feat/a')
         WHERE name = 'feat/b'",
        [],
    )
    .expect("seed cycle b -> a");

    stack_cmd(repo.path())
        .args(["doctor", "--porcelain"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"code\": \"cycle\""));

    stack_cmd(repo.path())
        .args(["doctor", "--fix"])
        .assert()
        .success();

    let output = stack_cmd(repo.path())
        .args(["doctor", "--porcelain"])
        .output()
        .expect("run doctor after fix");
    assert!(output.status.success());
    let json: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let has_cycle = json["issues"]
        .as_array()
        .expect("issues array")
        .iter()
        .any(|issue| issue["code"] == "cycle");
    assert!(!has_cycle, "expected cycle issues to be fixed");
}
