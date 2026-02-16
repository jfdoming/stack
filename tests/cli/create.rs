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

#[test]
fn create_before_inserts_new_branch_between_parent_and_child() {
    let repo = init_repo();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/parent"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/parent", "--name", "feat/child"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .args([
            "create",
            "--insert",
            "feat/child",
            "--name",
            "feat/mid",
            "--porcelain",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"created\": \"feat/mid\""))
        .stdout(predicate::str::contains("\"parent\": \"feat/parent\""))
        .stdout(predicate::str::contains("\"inserted_before\": \"feat/child\""));

    let db_path = repo.path().join(".git").join("stack.db");
    let conn = Connection::open(db_path).expect("open db");

    let mid_parent: String = conn
        .query_row(
            "SELECT p.name
             FROM branches c
             JOIN branches p ON p.id = c.parent_branch_id
             WHERE c.name = 'feat/mid'",
            [],
            |row| row.get(0),
        )
        .expect("mid parent");
    assert_eq!(mid_parent, "feat/parent");

    let child_parent: String = conn
        .query_row(
            "SELECT p.name
             FROM branches c
             JOIN branches p ON p.id = c.parent_branch_id
             WHERE c.name = 'feat/child'",
            [],
            |row| row.get(0),
        )
        .expect("child parent");
    assert_eq!(child_parent, "feat/mid");
}

#[cfg(unix)]
#[test]
fn create_before_refreshes_open_pr_bodies_for_affected_branches() {
    let repo = init_repo();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/parent"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/parent", "--name", "feat/child"])
        .assert()
        .success();

    let fake_bin = repo.path().join("fake-bin");
    let gh_log = repo.path().join("gh.log");
    fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        format!(
            "#!/usr/bin/env bash\necho \"$@\" >> '{}'\nif [[ \"$1\" == \"pr\" && \"$2\" == \"list\" ]]; then\n  echo '[{{\"number\":101,\"state\":\"OPEN\",\"baseRefName\":\"main\",\"headRefName\":\"feat/parent\",\"headRepositoryOwner\":{{\"login\":\"acme\"}},\"url\":\"https://github.com/acme/stack-test/pull/101\",\"body\":\"Parent user body\"}},{{\"number\":102,\"state\":\"OPEN\",\"baseRefName\":\"feat/parent\",\"headRefName\":\"feat/child\",\"headRepositoryOwner\":{{\"login\":\"acme\"}},\"url\":\"https://github.com/acme/stack-test/pull/102\",\"body\":\"Child user body\"}}]'\n  exit 0\nfi\nif [[ \"$1\" == \"pr\" && \"$2\" == \"edit\" ]]; then\n  exit 0\nfi\necho '[]'\n",
            gh_log.display()
        ),
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");

    let current_path = env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", fake_bin.display(), current_path);

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .args([
            "create",
            "--insert",
            "feat/child",
            "--name",
            "feat/mid",
        ])
        .assert()
        .success();

    let gh_calls = fs::read_to_string(&gh_log).expect("read gh call log");
    assert!(
        gh_calls.contains("pr edit 101 --body"),
        "expected parent PR body update, got: {gh_calls}"
    );
    assert!(
        gh_calls.contains("pr edit 102 --body"),
        "expected child PR body update, got: {gh_calls}"
    );
    assert!(
        gh_calls.contains("/tree/feat/mid"),
        "expected inserted branch reference in managed body updates, got: {gh_calls}"
    );
}

#[test]
fn create_insert_before_without_child_in_non_interactive_mode_assumes_only_viable_child() {
    let repo = init_repo();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/child"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .args(["create", "--insert", "--name", "feat/mid"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "assuming child branch 'feat/child' (only viable branch)",
        ));
}
