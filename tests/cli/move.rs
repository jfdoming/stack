#[test]
fn move_command_reparents_target_and_keeps_descendants_attached() {
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
        .args(["move", "feat/b", "--parent", "main", "--porcelain"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"action\": \"move\""))
        .stdout(predicate::str::contains("\"target\": \"feat/b\""))
        .stdout(predicate::str::contains("\"new_parent\": \"main\""));

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(db_path).expect("open db");

    let feat_b_parent: String = conn
        .query_row(
            "SELECT p.name
             FROM branches c
             JOIN branches p ON p.id = c.parent_branch_id
             WHERE c.name = 'feat/b'",
            [],
            |row| row.get(0),
        )
        .expect("feat/b parent");
    assert_eq!(feat_b_parent, "main");

    let feat_c_parent: String = conn
        .query_row(
            "SELECT p.name
             FROM branches c
             JOIN branches p ON p.id = c.parent_branch_id
             WHERE c.name = 'feat/c'",
            [],
            |row| row.get(0),
        )
        .expect("feat/c parent");
    assert_eq!(feat_c_parent, "feat/b");
}

#[test]
fn move_without_target_defaults_to_current_branch() {
    let repo = init_repo();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/a"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/other"])
        .assert()
        .success();

    run_git(repo.path(), &["checkout", "feat/a"]);

    stack_cmd(repo.path())
        .args(["move", "--parent", "feat/other"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "defaulting target branch to current branch 'feat/a'",
        ));

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(db_path).expect("open db");
    let feat_a_parent: String = conn
        .query_row(
            "SELECT p.name
             FROM branches c
             JOIN branches p ON p.id = c.parent_branch_id
             WHERE c.name = 'feat/a'",
            [],
            |row| row.get(0),
        )
        .expect("feat/a parent");
    assert_eq!(feat_a_parent, "feat/other");
}

#[test]
fn move_requires_parent_in_non_interactive_mode_when_omitted() {
    let repo = init_repo();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/a"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .args(["move", "feat/a"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "missing parent branch; usage: stack move [target] --parent <parent>",
        ));
}

#[test]
fn move_rejects_reparenting_onto_descendant() {
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
        .args(["move", "feat/a", "--parent", "feat/b"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("link would create a cycle"));
}

#[test]
fn move_rejects_reparenting_onto_nested_descendant() {
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
        .args(["move", "feat/a", "--parent", "feat/c"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("link would create a cycle"));
}
