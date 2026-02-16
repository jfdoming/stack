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
