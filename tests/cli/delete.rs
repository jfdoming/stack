#[cfg(unix)]
fn install_open_pr_delete_fixture(
    repo: &Path,
    name: &str,
    close_status: i32,
) -> (String, PathBuf) {
    let fake_bin = repo.join(format!("fake-bin-delete-{name}"));
    let gh_log = repo.join(format!("delete-{name}.log"));
    fs::create_dir_all(&fake_bin).expect("create fake bin");
    fs::write(&gh_log, "").expect("create gh log");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        format!(
            "#!/usr/bin/env bash\necho \"$@\" >> '{}'\nif [[ \"$1\" == \"pr\" && \"$2\" == \"list\" ]]; then\n  echo '[{{\"number\":42,\"state\":\"OPEN\",\"baseRefName\":\"main\",\"headRefName\":\"feat/victim\",\"headRepositoryOwner\":{{\"login\":\"acme\"}},\"url\":\"https://github.com/acme/stack-test/pull/42\"}}]'\n  exit 0\nfi\nif [[ \"$1\" == \"pr\" && \"$2\" == \"close\" ]]; then\n  if [[ {} -ne 0 ]]; then\n    echo 'forced PR close failure' >&2\n  fi\n  exit {}\nfi\necho '[]'\n",
            gh_log.display(),
            close_status,
            close_status
        ),
    )
    .expect("write fake gh");
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");
    let test_path = format!(
        "{}:{}",
        fake_bin.display(),
        env::var("PATH").unwrap_or_default()
    );
    (test_path, gh_log)
}

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

#[cfg(unix)]
#[test]
fn delete_keeps_open_pr_when_target_is_checked_out_in_linked_worktree() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/victim"])
        .assert()
        .success();
    let victim_head =
        git_ref_sha(repo.path(), "refs/heads/feat/victim").expect("victim branch head");
    run_git(repo.path(), &["checkout", "main"]);

    let linked_root = tempfile::tempdir().expect("linked worktree root");
    let linked = linked_root.path().join("victim");
    run_git(
        repo.path(),
        &[
            "worktree",
            "add",
            linked.to_str().expect("linked worktree path"),
            "feat/victim",
        ],
    );
    let (test_path, gh_log) = install_open_pr_delete_fixture(repo.path(), "linked", 0);

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["--yes", "delete", "feat/victim"])
        .assert()
        .failure();

    let calls = fs::read_to_string(gh_log).expect("read gh calls");
    assert!(
        !calls.lines().any(|line| line.starts_with("pr close ")),
        "the PR must remain open when a linked worktree prevents local deletion: {calls}"
    );
    assert_eq!(
        git_ref_sha(repo.path(), "refs/heads/feat/victim"),
        Some(victim_head)
    );
    let conn = Connection::open(repo.path().join(".git/stack.db")).expect("open stack db");
    let tracked: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM branches WHERE name = 'feat/victim'",
            [],
            |row| row.get(0),
        )
        .expect("count victim");
    assert_eq!(tracked, 1);
}

#[cfg(unix)]
#[test]
fn delete_restores_local_ref_when_post_delete_worktree_inspection_fails() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/victim"])
        .assert()
        .success();
    let victim_head =
        git_ref_sha(repo.path(), "refs/heads/feat/victim").expect("victim branch head");
    run_git(repo.path(), &["checkout", "main"]);

    let (test_path, gh_log) =
        install_open_pr_delete_fixture(repo.path(), "post-inspection-failure", 0);
    let fake_bin = repo.path().join("fake-bin-delete-post-inspection-failure");
    let inspection_count = repo.path().join("delete-worktree-inspection-count.log");
    fs::write(&inspection_count, "0\n").expect("create inspection count");
    let real_git = Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .expect("resolve real git");
    assert!(real_git.status.success());
    let real_git = String::from_utf8(real_git.stdout)
        .expect("utf8 git path")
        .trim()
        .to_string();
    let fake_git = fake_bin.join("git");
    fs::write(
        &fake_git,
        format!(
            "#!/usr/bin/env bash\nif [[ \"$1\" == \"worktree\" && \"$2\" == \"list\" ]]; then\n  count=$(< '{}')\n  count=$((count + 1))\n  echo \"$count\" > '{}'\n  if [[ \"$count\" -eq 2 ]]; then\n    echo 'forced post-delete worktree inspection failure' >&2\n    exit 17\n  fi\nfi\nexec '{}' \"$@\"\n",
            inspection_count.display(),
            inspection_count.display(),
            real_git
        ),
    )
    .expect("write fake git");
    fs::set_permissions(&fake_git, fs::Permissions::from_mode(0o755)).expect("chmod fake git");

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["--yes", "delete", "feat/victim"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "forced post-delete worktree inspection failure",
        ));

    assert_eq!(
        git_ref_sha(repo.path(), "refs/heads/feat/victim"),
        Some(victim_head),
        "the deleted ref must be restored when post-delete inspection fails"
    );
    let conn = Connection::open(repo.path().join(".git/stack.db")).expect("open stack db");
    let tracked: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM branches WHERE name = 'feat/victim'",
            [],
            |row| row.get(0),
        )
        .expect("count victim");
    assert_eq!(tracked, 1);
    let calls = fs::read_to_string(gh_log).expect("read gh calls");
    assert!(
        !calls.lines().any(|line| line.starts_with("pr close ")),
        "the PR must remain open when post-delete inspection fails: {calls}"
    );
}

#[cfg(unix)]
#[test]
fn delete_keeps_open_pr_when_current_branch_parent_is_missing() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/parent"])
        .assert()
        .success();
    stack_cmd(repo.path())
        .args([
            "create",
            "--parent",
            "feat/parent",
            "--name",
            "feat/victim",
        ])
        .assert()
        .success();
    let victim_head =
        git_ref_sha(repo.path(), "refs/heads/feat/victim").expect("victim branch head");
    run_git(repo.path(), &["branch", "-D", "feat/parent"]);
    let (test_path, gh_log) = install_open_pr_delete_fixture(repo.path(), "missing-parent", 0);

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["--yes", "delete", "feat/victim"])
        .assert()
        .failure();

    let calls = fs::read_to_string(gh_log).expect("read gh calls");
    assert!(
        !calls.lines().any(|line| line.starts_with("pr close ")),
        "the PR must remain open when checkout of the parent fails: {calls}"
    );
    assert_eq!(
        git_ref_sha(repo.path(), "refs/heads/feat/victim"),
        Some(victim_head)
    );
    let current = Command::new("git")
        .current_dir(repo.path())
        .args(["branch", "--show-current"])
        .output()
        .expect("read current branch");
    assert!(current.status.success());
    assert_eq!(
        String::from_utf8(current.stdout)
            .expect("utf8 current branch")
            .trim(),
        "feat/victim"
    );
    let conn = Connection::open(repo.path().join(".git/stack.db")).expect("open stack db");
    let parent: String = conn
        .query_row(
            "SELECT p.name
             FROM branches c
             JOIN branches p ON p.id = c.parent_branch_id
             WHERE c.name = 'feat/victim'",
            [],
            |row| row.get(0),
        )
        .expect("victim parent");
    assert_eq!(parent, "feat/parent");
}

#[cfg(unix)]
#[test]
fn delete_keeps_open_pr_and_restores_local_ref_when_database_splice_fails() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/victim"])
        .assert()
        .success();
    let victim_head =
        git_ref_sha(repo.path(), "refs/heads/feat/victim").expect("victim branch head");
    stack_cmd(repo.path())
        .args([
            "create",
            "--parent",
            "feat/victim",
            "--name",
            "feat/child",
        ])
        .assert()
        .success();
    run_git(
        repo.path(),
        &["config", "branch.feat/victim.remote", "origin"],
    );
    run_git(
        repo.path(),
        &[
            "config",
            "branch.feat/victim.merge",
            "refs/heads/feat/victim",
        ],
    );

    let conn = Connection::open(repo.path().join(".git/stack.db")).expect("open stack db");
    conn.execute_batch(
        "CREATE TRIGGER fail_victim_splice
         BEFORE DELETE ON branches
         WHEN OLD.name = 'feat/victim'
         BEGIN
             SELECT RAISE(ABORT, 'forced victim splice failure');
         END;",
    )
    .expect("install splice failure");
    drop(conn);
    run_git(repo.path(), &["checkout", "feat/victim"]);
    let (test_path, gh_log) = install_open_pr_delete_fixture(repo.path(), "splice-failure", 0);

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["--yes", "delete", "feat/victim"])
        .assert()
        .failure();

    let calls = fs::read_to_string(gh_log).expect("read gh calls");
    assert!(
        !calls.lines().any(|line| line.starts_with("pr close ")),
        "the PR must remain open when database splicing fails: {calls}"
    );
    assert_eq!(
        git_ref_sha(repo.path(), "refs/heads/feat/victim"),
        Some(victim_head),
        "the deleted local ref must be restored"
    );
    let current = Command::new("git")
        .current_dir(repo.path())
        .args(["branch", "--show-current"])
        .output()
        .expect("read current branch");
    assert!(current.status.success());
    assert_eq!(
        String::from_utf8(current.stdout)
            .expect("utf8 current branch")
            .trim(),
        "feat/victim",
        "the starting checkout must be restored"
    );
    let conn = Connection::open(repo.path().join(".git/stack.db")).expect("open stack db");
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
    assert_eq!(child_parent, "feat/victim");
    let remote = Command::new("git")
        .current_dir(repo.path())
        .args(["config", "--get", "branch.feat/victim.remote"])
        .output()
        .expect("read restored branch remote");
    assert!(remote.status.success());
    assert_eq!(
        String::from_utf8(remote.stdout)
            .expect("utf8 branch remote")
            .trim(),
        "origin"
    );
}

#[cfg(unix)]
#[test]
fn delete_fails_without_mutation_when_branch_configuration_cannot_be_removed() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/victim"])
        .assert()
        .success();
    let victim_head =
        git_ref_sha(repo.path(), "refs/heads/feat/victim").expect("victim branch head");
    run_git(
        repo.path(),
        &["config", "branch.feat/victim.remote", "origin"],
    );
    run_git(
        repo.path(),
        &[
            "config",
            "branch.feat/victim.merge",
            "refs/heads/feat/victim",
        ],
    );
    let (test_path, gh_log) = install_open_pr_delete_fixture(repo.path(), "config-lock", 0);
    fs::write(repo.path().join(".git/config.lock"), "held\n").expect("hold config lock");

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["--yes", "delete", "feat/victim"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("could not lock config file"));

    assert_eq!(
        git_ref_sha(repo.path(), "refs/heads/feat/victim"),
        Some(victim_head)
    );
    let current = Command::new("git")
        .current_dir(repo.path())
        .args(["branch", "--show-current"])
        .output()
        .expect("read restored checkout");
    assert!(current.status.success());
    assert_eq!(
        String::from_utf8(current.stdout)
            .expect("utf8 current branch")
            .trim(),
        "feat/victim"
    );
    let conn = Connection::open(repo.path().join(".git/stack.db")).expect("open stack db");
    let tracked: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM branches WHERE name = 'feat/victim'",
            [],
            |row| row.get(0),
        )
        .expect("count victim");
    assert_eq!(tracked, 1);
    let remote = Command::new("git")
        .current_dir(repo.path())
        .args(["config", "--get", "branch.feat/victim.remote"])
        .output()
        .expect("read retained branch remote");
    assert!(remote.status.success());
    assert_eq!(
        String::from_utf8(remote.stdout)
            .expect("utf8 branch remote")
            .trim(),
        "origin"
    );
    let calls = fs::read_to_string(gh_log).expect("read gh calls");
    assert!(
        !calls.lines().any(|line| line.starts_with("pr close ")),
        "the PR must remain open when branch configuration cleanup fails: {calls}"
    );
}

#[cfg(unix)]
#[test]
fn delete_refuses_to_remove_a_branch_that_advances_during_pr_lookup() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/victim"])
        .assert()
        .success();
    let original_head =
        git_ref_sha(repo.path(), "refs/heads/feat/victim").expect("original victim head");
    run_git(repo.path(), &["checkout", "main"]);
    run_git(repo.path(), &["checkout", "-b", "concurrent-source"]);
    fs::write(repo.path().join("concurrent.txt"), "concurrent\n").expect("write concurrent change");
    run_git(repo.path(), &["add", "concurrent.txt"]);
    run_git(repo.path(), &["commit", "-m", "concurrent change"]);
    let concurrent_head =
        git_ref_sha(repo.path(), "refs/heads/concurrent-source").expect("concurrent head");
    run_git(repo.path(), &["checkout", "main"]);
    run_git(repo.path(), &["branch", "-D", "concurrent-source"]);
    run_git(repo.path(), &["checkout", "feat/victim"]);
    assert_ne!(original_head, concurrent_head);

    let fake_bin = repo.path().join("fake-bin-delete-ref-race");
    let gh_log = repo.path().join("delete-ref-race.log");
    fs::create_dir_all(&fake_bin).expect("create fake bin");
    fs::write(&gh_log, "").expect("create gh log");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        format!(
            "#!/usr/bin/env bash\necho \"$@\" >> '{}'\nif [[ \"$1\" == \"pr\" && \"$2\" == \"list\" ]]; then\n  git update-ref refs/heads/feat/victim '{}'\n  echo '[{{\"number\":42,\"state\":\"OPEN\",\"baseRefName\":\"main\",\"headRefName\":\"feat/victim\",\"headRepositoryOwner\":{{\"login\":\"acme\"}},\"url\":\"https://github.com/acme/stack-test/pull/42\"}}]'\n  exit 0\nfi\nif [[ \"$1\" == \"pr\" && \"$2\" == \"close\" ]]; then\n  exit 0\nfi\necho '[]'\n",
            gh_log.display(),
            concurrent_head
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
        .failure()
        .stderr(predicate::str::contains(
            "branch changed before deletion",
        ));

    assert_eq!(
        git_ref_sha(repo.path(), "refs/heads/feat/victim"),
        Some(concurrent_head)
    );
    let current = Command::new("git")
        .current_dir(repo.path())
        .args(["branch", "--show-current"])
        .output()
        .expect("read current branch");
    assert!(current.status.success());
    assert_eq!(
        String::from_utf8(current.stdout)
            .expect("utf8 current branch")
            .trim(),
        "feat/victim",
        "a failed conditional deletion must restore the original checkout"
    );
    let calls = fs::read_to_string(gh_log).expect("read gh calls");
    assert!(
        !calls.lines().any(|line| line.starts_with("pr close ")),
        "a concurrently advanced branch must not authorize PR closure: {calls}"
    );
    let conn = Connection::open(repo.path().join(".git/stack.db")).expect("open stack db");
    let tracked: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM branches WHERE name = 'feat/victim'",
            [],
            |row| row.get(0),
        )
        .expect("count victim");
    assert_eq!(tracked, 1);
}

#[cfg(unix)]
#[test]
fn delete_reports_partial_completion_when_pr_close_outcome_is_uncertain() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/victim"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "main"]);
    let (test_path, gh_log) =
        install_open_pr_delete_fixture(repo.path(), "close-failure", 9);

    stack_cmd(repo.path())
        .env("PATH", test_path)
        .args(["--yes", "delete", "feat/victim"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "deleted local branch 'feat/victim' and spliced stack metadata",
        ))
        .stderr(predicate::str::contains(
            "could not confirm closure of PR #42",
        ));

    assert!(git_ref_sha(repo.path(), "refs/heads/feat/victim").is_none());
    let conn = Connection::open(repo.path().join(".git/stack.db")).expect("open stack db");
    let tracked: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM branches WHERE name = 'feat/victim'",
            [],
            |row| row.get(0),
        )
        .expect("count victim");
    assert_eq!(tracked, 0);
    let calls = fs::read_to_string(gh_log).expect("read gh calls");
    assert!(calls.lines().any(|line| line.starts_with("pr close 42")));
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
        .env("PATH", &test_path)
        .args(["delete", "feat/victim", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "would delete branch 'feat/victim' (PR: #77) and splice children under 'main'",
        ))
        .stdout(predicate::str::contains("Number(").not());

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
fn delete_does_not_delegate_local_branch_deletion_to_gh() {
    let repo = init_repo();
    stack_cmd(repo.path())
        .args(["create", "--parent", "main", "--name", "feat/victim"])
        .assert()
        .success();
    run_git(repo.path(), &["checkout", "main"]);

    let fake_bin = repo.path().join("fake-bin-delete-ownership");
    let gh_log = repo.path().join("delete-ownership.log");
    fs::create_dir_all(&fake_bin).expect("create fake bin");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        format!(
            "#!/usr/bin/env bash\necho \"$@\" >> '{}'\nif [[ \"$1\" == \"pr\" && \"$2\" == \"list\" ]]; then\n  echo '[{{\"number\":42,\"state\":\"OPEN\",\"headRefName\":\"feat/victim\",\"headRepositoryOwner\":{{\"login\":\"acme\"}},\"url\":\"https://github.com/acme/stack-test/pull/42\"}}]'\n  exit 0\nfi\nif [[ \"$1\" == \"pr\" && \"$2\" == \"close\" ]]; then\n  if git show-ref --verify --quiet refs/heads/feat/victim; then\n    echo 'local branch still existed when PR close ran' >&2\n    exit 9\n  fi\n  if [[ \"$*\" == *\"--delete-branch\"* ]]; then\n    git branch -D feat/victim\n  fi\n  exit 0\nfi\necho '[]'\n",
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

    assert!(git_ref_sha(repo.path(), "refs/heads/feat/victim").is_none());
    let conn = Connection::open(repo.path().join(".git/stack.db")).expect("open stack db");
    let tracked: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM branches WHERE name = 'feat/victim'",
            [],
            |row| row.get(0),
        )
        .expect("count victim records");
    assert_eq!(tracked, 0);

    let calls = fs::read_to_string(&gh_log).expect("read gh calls");
    let close = calls
        .lines()
        .find(|line| line.starts_with("pr close 42"))
        .expect("close call");
    assert!(
        !close.contains("--delete-branch"),
        "stack must own local deletion so metadata cleanup can follow: {close}"
    );
}

#[cfg(unix)]
#[test]
fn delete_skips_close_for_terminal_prs() {
    for state in ["MERGED", "CLOSED"] {
        let repo = init_repo();
        stack_cmd(repo.path())
            .args(["create", "--parent", "main", "--name", "feat/victim"])
            .assert()
            .success();
        let head = git_ref_sha(repo.path(), "refs/heads/feat/victim").expect("victim head");
        run_git(repo.path(), &["checkout", "main"]);

        let fake_bin = repo.path().join(format!("fake-bin-delete-{}", state.to_lowercase()));
        let gh_log = repo.path().join(format!("delete-{}.log", state.to_lowercase()));
        fs::create_dir_all(&fake_bin).expect("create fake bin");
        let fake_gh = fake_bin.join("gh");
        fs::write(
            &fake_gh,
            format!(
                "#!/usr/bin/env bash\necho \"$@\" >> '{}'\nif [[ \"$1\" == \"pr\" && \"$2\" == \"list\" ]]; then\n  echo '[{{\"number\":42,\"state\":\"{}\",\"headRefName\":\"feat/victim\",\"headRefOid\":\"{}\",\"headRepositoryOwner\":{{\"login\":\"acme\"}},\"url\":\"https://github.com/acme/stack-test/pull/42\"}}]'\n  exit 0\nfi\nif [[ \"$1\" == \"pr\" && \"$2\" == \"close\" ]]; then\n  echo 'terminal PR must not be closed again' >&2\n  exit 9\nfi\necho '[]'\n",
                gh_log.display(),
                state,
                head
            ),
        )
        .expect("write fake gh");
        fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755))
            .expect("chmod fake gh");
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

        let calls = fs::read_to_string(&gh_log).expect("read gh calls");
        assert!(
            !calls.lines().any(|line| line.starts_with("pr close ")),
            "{state} PR was closed again: {calls}"
        );
        assert!(git_ref_sha(repo.path(), "refs/heads/feat/victim").is_none());
    }
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
    fs::write(&gh_log, "").expect("create gh log");
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
        ))
        .stdout(predicate::str::contains(
            "would delete branch 'feat/a' (PR: none) and splice children under 'main'",
        ))
        .stdout(predicate::str::contains("Null").not())
        .stdout(predicate::str::contains("\"feat/a\"").not());
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
