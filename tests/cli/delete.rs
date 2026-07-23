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
fn delete_rejects_the_configured_base_branch_without_mutation() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/a"])
        .assert()
        .success();

    stack_cmd(repo.path())
        .args(["--yes", "delete", "main"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot delete base branch 'main'"));

    assert!(git_ref_sha(repo.path(), "refs/heads/main").is_some());
    let conn = Connection::open(repo.path().join(".git/stack.db")).expect("open stack db");
    let base_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM branches WHERE name = 'main'",
            [],
            |row| row.get(0),
        )
        .expect("count base records");
    assert_eq!(base_count, 1);
}

#[cfg(unix)]
#[test]
fn delete_verifies_cached_pr_head_identity_and_preserves_repository_scope() {
    let repo = init_repo();
    run_git(
        repo.path(),
        &["remote", "set-url", "origin", "git@github.com:alice/stack-test.git"],
    );
    run_git(
        repo.path(),
        &["remote", "add", "upstream", "git@github.com:acme/stack-test.git"],
    );
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/victim"])
        .assert()
        .success();
    run_git(
        repo.path(),
        &["config", "branch.feat/victim.remote", "origin"],
    );

    let conn = Connection::open(repo.path().join(".git/stack.db")).expect("open stack db");
    conn.execute(
        "UPDATE branches
         SET cached_pr_number = 42, cached_pr_state = 'open'
         WHERE name = 'feat/victim'",
        [],
    )
    .expect("seed cached PR");
    drop(conn);

    let fake_bin = repo.path().join("fake-bin-pr-identity");
    let gh_log = repo.path().join("pr-identity.log");
    fs::create_dir_all(&fake_bin).expect("create fake bin");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        format!(
            "#!/usr/bin/env bash\necho \"$@\" >> '{}'\nif [[ \"$1\" == \"pr\" && \"$2\" == \"view\" && \"$3\" == \"42\" ]]; then\n  if [[ \"$*\" == *\"--repo acme/stack-test\"* ]]; then\n    echo '{{\"number\":42,\"state\":\"OPEN\",\"headRefName\":\"feat/victim\",\"headRepositoryOwner\":{{\"login\":\"mallory\"}},\"url\":\"https://github.com/acme/stack-test/pull/42\"}}'\n  else\n    echo '{{\"number\":42,\"state\":\"OPEN\",\"headRefName\":\"feat/victim\",\"headRepositoryOwner\":{{\"login\":\"alice\"}},\"url\":\"https://github.com/alice/stack-test/pull/42\"}}'\n  fi\n  exit 0\nfi\nif [[ \"$1\" == \"pr\" && \"$2\" == \"list\" && \"$*\" == *\"--repo acme/stack-test\"* ]]; then\n  echo '[{{\"number\":77,\"state\":\"OPEN\",\"headRefName\":\"feat/victim\",\"headRepositoryOwner\":{{\"login\":\"alice\"}},\"url\":\"https://github.com/acme/stack-test/pull/77\"}}]'\n  exit 0\nfi\nif [[ \"$1\" == \"pr\" && \"$2\" == \"close\" ]]; then\n  if [[ \"$3\" == \"77\" && \"$*\" == *\"--repo acme/stack-test\"* ]]; then\n    exit 0\n  fi\n  echo 'refusing stale, unscoped, or wrong-repository close' >&2\n  exit 9\nfi\necho '[]'\n",
            gh_log.display()
        ),
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");
    let test_path = format!(
        "{}:{}",
        fake_bin.display(),
        env::var("PATH").unwrap_or_default()
    );

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["--yes", "delete", "feat/victim"])
        .assert()
        .success();

    let gh_calls = fs::read_to_string(&gh_log).expect("read gh log");
    assert!(gh_calls.contains("pr view 42"));
    assert!(gh_calls.contains("--repo acme/stack-test"));
    assert!(gh_calls.contains("pr list --head alice:feat/victim"));
    assert!(
        gh_calls
            .lines()
            .any(|line| line.starts_with("pr close 77")
                && line.contains("--repo acme/stack-test")),
        "expected repository-scoped close, got: {gh_calls}"
    );
}

#[cfg(unix)]
#[test]
fn delete_does_not_mutate_a_terminal_pr_when_the_local_branch_is_missing() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/reused"])
        .assert()
        .success();
    fs::write(repo.path().join("old.txt"), "old branch\n").expect("write old branch");
    run_git(repo.path(), &["add", "old.txt"]);
    run_git(repo.path(), &["commit", "-m", "old branch incarnation"]);
    let old_head = git_ref_sha(repo.path(), "feat/reused").expect("old branch head");
    run_git(repo.path(), &["checkout", "main"]);
    run_git(repo.path(), &["branch", "-D", "feat/reused"]);

    let fake_bin = repo.path().join("fake-bin-missing-local-pr");
    let gh_log = repo.path().join("missing-local-pr.log");
    fs::create_dir_all(&fake_bin).expect("create fake bin");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        format!(
            "#!/usr/bin/env bash\necho \"$@\" >> '{}'\nif [[ \"$1\" == \"pr\" && \"$2\" == \"list\" ]]; then\n  echo '[{{\"number\":11,\"state\":\"MERGED\",\"baseRefName\":\"main\",\"headRefName\":\"feat/reused\",\"headRefOid\":\"{}\",\"mergeCommit\":{{\"oid\":\"{}\"}},\"headRepositoryOwner\":{{\"login\":\"acme\"}},\"url\":\"https://github.com/acme/stack-test/pull/11\",\"body\":\"\"}}]'\n  exit 0\nfi\nif [[ \"$1\" == \"pr\" && \"$2\" == \"close\" ]]; then\n  exit 0\nfi\necho '[]'\n",
            gh_log.display(),
            old_head,
            old_head
        ),
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");
    let test_path = format!(
        "{}:{}",
        fake_bin.display(),
        env::var("PATH").unwrap_or_default()
    );

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["--yes", "delete", "feat/reused"])
        .assert()
        .failure();

    let calls = fs::read_to_string(&gh_log).expect("read gh calls");
    assert!(
        !calls.lines().any(|line| line.starts_with("pr close ")),
        "a name-only terminal PR match must not authorize remote deletion: {calls}"
    );
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
