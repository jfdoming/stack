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
        .stdout(predicate::str::contains("\"create_url\": \"\""));

    let current = Command::new("git")
        .current_dir(repo.path())
        .args(["branch", "--show-current"])
        .output()
        .expect("read current branch");
    assert!(current.status.success());
    assert_eq!(String::from_utf8_lossy(&current.stdout).trim(), "feat/one");

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
