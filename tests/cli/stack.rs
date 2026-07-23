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
fn stack_output_redacts_credentials_from_remote_links() {
    let repo = init_repo();
    run_git(
        repo.path(),
        &[
            "remote",
            "set-url",
            "origin",
            "https://build-user:super-secret@github.com/acme/stack-test.git",
        ],
    );
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/credential-link"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "https://github.com/acme/stack-test/compare/main...feat/credential-link",
        ))
        .stdout(predicate::str::contains("super-secret").not())
        .stdout(predicate::str::contains("build-user").not());
}

#[test]
fn stack_metadata_is_shared_across_linked_worktrees() {
    let repo = init_repo_without_origin();
    stack_cmd(repo.path()).assert().success();
    run_git(repo.path(), &["branch", "feat/linked"]);

    let worktree_root = tempfile::tempdir().expect("worktree tempdir");
    let linked = worktree_root.path().join("linked");
    run_git(
        repo.path(),
        &[
            "worktree",
            "add",
            linked.to_str().expect("linked worktree path"),
            "feat/linked",
        ],
    );

    stack_cmd(&linked)
        .args(["track", "feat/linked", "--parent", "main"])
        .assert()
        .success();
    let create_output = stack_cmd(&linked)
        .args([
            "create",
            "--parent",
            "feat/linked",
            "--name",
            "feat/from-linked",
            "--porcelain",
        ])
        .output()
        .expect("create branch from linked worktree");
    assert!(create_output.status.success());
    let create_json: Value =
        serde_json::from_slice(&create_output.stdout).expect("valid create JSON");
    let expected_db = fs::canonicalize(repo.path().join(".git/stack.db"))
        .expect("canonical shared database path");
    assert_eq!(
        create_json["db"],
        expected_db.display().to_string()
    );

    stack_cmd(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("feat/linked"))
        .stdout(predicate::str::contains("feat/from-linked"));
}

#[test]
fn linked_worktree_migrates_a_legacy_database_when_shared_is_missing() {
    let repo = init_repo_without_origin();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/legacy"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "main"]);
    run_git(repo.path(), &["branch", "feat/linked"]);

    let worktree_root = tempfile::tempdir().expect("worktree tempdir");
    let linked = worktree_root.path().join("linked");
    run_git(
        repo.path(),
        &[
            "worktree",
            "add",
            linked.to_str().expect("linked worktree path"),
            "feat/linked",
        ],
    );

    let git_dir_output = Command::new("git")
        .current_dir(&linked)
        .args(["rev-parse", "--path-format=absolute", "--git-dir"])
        .output()
        .expect("resolve linked git dir");
    assert!(git_dir_output.status.success());
    let linked_git_dir = PathBuf::from(
        String::from_utf8(git_dir_output.stdout)
            .expect("utf8 git dir")
            .trim(),
    );
    let shared_db = repo.path().join(".git/stack.db");
    let legacy_db = linked_git_dir.join("stack.db");
    fs::rename(&shared_db, &legacy_db).expect("seed legacy worktree database");

    stack_cmd(&linked)
        .assert()
        .success()
        .stdout(predicate::str::contains("feat/legacy"));
    stack_cmd(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("feat/legacy"));
    assert!(shared_db.exists());
    assert!(!legacy_db.exists());
}

#[test]
fn stack_initializes_non_main_repo_base_from_current_branch() {
    let repo = init_repo_on_branch("trunk");
    stack_cmd(repo.path()).assert().success();

    assert_eq!(stored_base_branch(repo.path()), "trunk");
    assert_eq!(stored_base_source(repo.path()), "local_convention");
}

#[test]
fn stack_ignores_a_dangling_remote_head_when_discovering_the_base() {
    let repo = init_repo_on_branch("trunk");
    run_git(
        repo.path(),
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/ghost",
        ],
    );

    stack_cmd(repo.path()).assert().success();
    assert_eq!(stored_base_branch(repo.path()), "trunk");
    assert_eq!(stored_base_source(repo.path()), "local_convention");
}

#[test]
fn stack_does_not_promote_a_dangling_remote_head_with_a_local_namesake() {
    let repo = init_repo_on_branch("production");
    run_git(repo.path(), &["checkout", "-b", "feat/work"]);
    stack_cmd(repo.path()).assert().success();
    assert_eq!(stored_base_branch(repo.path()), "feat/work");
    assert_eq!(stored_base_source(repo.path()), "current_branch");

    run_git(
        repo.path(),
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/production",
        ],
    );

    stack_cmd(repo.path()).assert().success();
    assert_eq!(stored_base_branch(repo.path()), "feat/work");
    assert_eq!(stored_base_source(repo.path()), "current_branch");
}

#[test]
fn stack_repairs_a_missing_cached_base_when_remote_head_becomes_known() {
    let repo = init_repo_on_branch("trunk");
    stack_cmd(repo.path()).assert().success();

    let conn = Connection::open(repo.path().join(".git/stack.db")).expect("open stack db");
    conn.execute("UPDATE repo_meta SET base_branch = 'main' WHERE id = 1", [])
        .expect("seed stale base");
    drop(conn);

    run_git(
        repo.path(),
        &["update-ref", "refs/remotes/origin/trunk", "trunk"],
    );
    run_git(
        repo.path(),
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/trunk",
        ],
    );

    stack_cmd(repo.path()).assert().success();
    assert_eq!(stored_base_branch(repo.path()), "trunk");
}

#[test]
fn stack_preserves_an_existing_cached_base_when_remote_head_differs() {
    let repo = init_repo_without_origin();
    stack_cmd(repo.path()).assert().success();
    run_git(repo.path(), &["branch", "trunk"]);
    run_git(
        repo.path(),
        &["update-ref", "refs/remotes/origin/trunk", "trunk"],
    );
    run_git(
        repo.path(),
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/trunk",
        ],
    );

    stack_cmd(repo.path()).assert().success();
    assert_eq!(stored_base_branch(repo.path()), "main");
    assert_eq!(stored_base_source(repo.path()), "local_convention");
}

#[test]
fn stack_replaces_a_provisional_current_branch_base_with_remote_head() {
    let repo = init_repo_on_branch("production");
    run_git(repo.path(), &["checkout", "-b", "feat/work"]);

    stack_cmd(repo.path()).assert().success();
    assert_eq!(stored_base_branch(repo.path()), "feat/work");
    assert_eq!(stored_base_source(repo.path()), "current_branch");

    run_git(
        repo.path(),
        &[
            "update-ref",
            "refs/remotes/origin/production",
            "production",
        ],
    );
    run_git(
        repo.path(),
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/production",
        ],
    );

    stack_cmd(repo.path()).assert().success();
    assert_eq!(stored_base_branch(repo.path()), "production");
    assert_eq!(stored_base_source(repo.path()), "remote_head");
}

fn stored_base_branch(repo: &Path) -> String {
    let conn = Connection::open(repo.join(".git/stack.db")).expect("open stack db");
    conn.query_row("SELECT base_branch FROM repo_meta WHERE id = 1", [], |row| {
        row.get(0)
    })
    .expect("read base branch")
}

fn stored_base_source(repo: &Path) -> String {
    let conn = Connection::open(repo.join(".git/stack.db")).expect("open stack db");
    conn.query_row(
        "SELECT base_branch_source FROM repo_meta WHERE id = 1",
        [],
        |row| row.get(0),
    )
    .expect("read base branch source")
}

#[test]
fn stack_up_and_down_switch_between_parent_and_child() {
    let repo = init_repo_without_origin();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/parent"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/parent", "--name", "feat/child"])
        .assert()
        .success();

    stack_cmd(repo.path()).args(["down"]).assert().success();
    assert_eq!(current_branch(repo.path()), "feat/parent");

    stack_cmd(repo.path()).args(["up"]).assert().success();
    assert_eq!(current_branch(repo.path()), "feat/child");
}

#[test]
fn stack_top_and_bottom_switch_to_stack_extremes() {
    let repo = init_repo_without_origin();

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

    stack_cmd(repo.path()).args(["bottom"]).assert().success();
    assert_eq!(current_branch(repo.path()), "feat/a");

    stack_cmd(repo.path()).args(["top"]).assert().success();
    assert_eq!(current_branch(repo.path()), "feat/c");
}

#[test]
fn stack_navigation_excludes_base_branch() {
    let repo = init_repo_without_origin();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/root"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "main"]);

    stack_cmd(repo.path())
        .args(["up"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("is not part of stack navigation"));

    run_git(repo.path(), &["checkout", "feat/root"]);

    stack_cmd(repo.path())
        .args(["down"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("has no parent branch in the stack"));
}

#[test]
fn stack_up_requires_disambiguation_when_branch_has_multiple_children() {
    let repo = init_repo_without_origin();

    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/root"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/root", "--name", "feat/a"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "feat/root"]);
    stack_cmd(repo.path())
        .args(["create", "--parent", "feat/root", "--name", "feat/b"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "feat/root"]);

    stack_cmd(repo.path())
        .args(["up"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("has multiple child branches"));
}

fn current_branch(repo: &std::path::Path) -> String {
    let output = Command::new("git")
        .current_dir(repo)
        .args(["branch", "--show-current"])
        .output()
        .expect("read current branch");
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .expect("utf8")
        .trim()
        .to_string()
}
